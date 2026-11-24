//! Moteur de transferts SFTP (jalon M5) : file d'attente, workers parallèles,
//! progression/débit/ETA, pause/reprise, **reprise après interruption**,
//! dossiers récursifs, permissions/dates préservées.
//!
//! Principes :
//! - Un job = une sélection (fichier ou dossier). Les dossiers sont développés
//!   en liste ordonnée (dossiers avant contenu) ; les fichiers d'un job sont
//!   traités séquentiellement, le parallélisme est ENTRE jobs (défaut 3, max 8).
//! - Écriture dans `<destination>.part` puis renommage atomique à la fin :
//!   une destination n'est jamais à moitié écrite.
//! - Reprise : si `.part` existe (taille ≤ source), on reprend à cet offset
//!   après avoir comparé les derniers 64 Kio des deux côtés ; discordance →
//!   on repart de zéro (source modifiée ou `.part` corrompu).
//! - Pause = arrêt propre en conservant le `.part` ; Reprendre relance le job.
//!   Annuler supprime le `.part` du fichier en cours.
//! - Intégrité : taille vérifiée à la fin (SFTP v3 n'a pas de hash standard).
//! - Liens symboliques : ignorés par défaut ; `Follow` les suit (profondeur ≤ 64).

use std::collections::HashMap;
use std::future::Future;
use std::io::SeekFrom;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{Semaphore, mpsc};

use crate::error::{Result, SshError};
use crate::session::SessionManager;
use crate::sftp::{base_name, is_fatal, join};

/// Taille des blocs lus/écrits.
const CHUNK: usize = 64 * 1024;
/// Octets comparés avant de reprendre sur un `.part`.
const TAIL_CHECK: u64 = 64 * 1024;
/// Profondeur max de parcours (boucles de liens suivis).
const MAX_DEPTH: usize = 64;
/// Intervalle minimal entre deux événements de progression.
const EMIT_EVERY: Duration = Duration::from_millis(200);
/// Parallélisme borné par le produit.
pub const MAX_PARALLEL: usize = 8;

const CTL_RUN: u8 = 0;
const CTL_PAUSE: u8 = 1;
const CTL_CANCEL: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Upload,
    Download,
}

/// Que faire si la destination existe déjà (fichier complet, pas `.part`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Conflict {
    #[default]
    Skip,
    Overwrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SymlinkPolicy {
    #[default]
    Skip,
    Follow,
}

/// Demande de transfert. `source` est un chemin LOCAL (envoi) ou DISTANT
/// (téléchargement) ; `dest_dir` le répertoire de l'autre côté. La destination
/// finale est `dest_dir/<nom de la source>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferSpec {
    pub session: String,
    pub direction: Direction,
    pub source: String,
    pub dest_dir: String,
    #[serde(default)]
    pub conflict: Conflict,
    #[serde(default)]
    pub symlinks: SymlinkPolicy,
    /// Préserver permissions et date de modification (défaut : oui).
    #[serde(default = "default_true")]
    pub preserve: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferState {
    Queued,
    Running,
    Paused,
    Done,
    Error,
    Cancelled,
}

/// Instantané d'un job (liste et événements).
#[derive(Debug, Clone, Serialize)]
pub struct TransferInfo {
    pub id: u64,
    pub session: String,
    pub direction: Direction,
    pub source: String,
    pub dest_dir: String,
    pub name: String,
    pub state: TransferState,
    pub error: Option<String>,
    pub done: u64,
    pub total: u64,
    pub files_done: u64,
    pub files_total: u64,
    /// Octets/s (moyenne depuis le démarrage/la reprise).
    pub bps: u64,
    pub eta_s: Option<u64>,
    /// Fichier en cours.
    pub current: Option<String>,
}

#[derive(Debug, Clone)]
pub enum TransferEvent {
    Update(TransferInfo),
}

struct Job {
    id: u64,
    spec: TransferSpec,
    ctl: AtomicU8,
    /// Incrémenté à chaque (re)lancement : une exécution périmée s'arrête.
    generation: AtomicU64,
    done: AtomicU64,
    total: AtomicU64,
    files_done: AtomicU64,
    files_total: AtomicU64,
    /// Octets déjà présents au démarrage de l'exécution (reprise) : exclus du débit.
    base: AtomicU64,
    started: Mutex<Option<Instant>>,
    last_emit: Mutex<Option<Instant>>,
    state: Mutex<TransferState>,
    error: Mutex<Option<String>>,
    current: Mutex<Option<String>>,
}

impl Job {
    fn snapshot(&self) -> TransferInfo {
        let done = self.done.load(Ordering::Relaxed);
        let total = self.total.load(Ordering::Relaxed);
        let state = *self.state.lock().unwrap();
        let bps = match *self.started.lock().unwrap() {
            Some(t0) if state == TransferState::Running => {
                let secs = t0.elapsed().as_secs_f64().max(0.001);
                let moved = done.saturating_sub(self.base.load(Ordering::Relaxed));
                (moved as f64 / secs) as u64
            }
            _ => 0,
        };
        let eta_s = (bps > 0 && total > done).then(|| (total - done) / bps);
        TransferInfo {
            id: self.id,
            session: self.spec.session.clone(),
            direction: self.spec.direction,
            source: self.spec.source.clone(),
            dest_dir: self.spec.dest_dir.clone(),
            name: base_name_any(&self.spec.source),
            state,
            error: self.error.lock().unwrap().clone(),
            done,
            total,
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total: self.files_total.load(Ordering::Relaxed),
            bps,
            eta_s,
            current: self.current.lock().unwrap().clone(),
        }
    }
}

/// Nom de base d'un chemin local ou distant.
fn base_name_any(path: &str) -> String {
    base_name(path).to_string()
}

enum Flow {
    Continue,
    Paused,
    Cancelled,
}

struct Item {
    src: String,
    dst: String,
    size: u64,
    /// Bits de permission (0o7777).
    mode: u32,
    mtime: u32,
    is_dir: bool,
}

pub struct TransferManager {
    sessions: Arc<SessionManager>,
    jobs: Mutex<HashMap<u64, Arc<Job>>>,
    next_id: AtomicU64,
    permits: Arc<Semaphore>,
    parallel: AtomicUsize,
    events: mpsc::UnboundedSender<TransferEvent>,
}

impl TransferManager {
    pub fn new(
        sessions: Arc<SessionManager>,
        parallel: usize,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<TransferEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let mgr = Arc::new(Self {
            sessions,
            jobs: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            permits: Arc::new(Semaphore::new(parallel.clamp(1, MAX_PARALLEL))),
            parallel: AtomicUsize::new(parallel.clamp(1, MAX_PARALLEL)),
            events: tx,
        });
        (mgr, rx)
    }

    /// Ajuste le parallélisme (1..=8) ; les jobs en cours ne sont pas interrompus.
    pub fn set_parallel(&self, n: usize) {
        let n = n.clamp(1, MAX_PARALLEL);
        let current = self.parallel.swap(n, Ordering::SeqCst);
        if n > current {
            self.permits.add_permits(n - current);
        } else if n < current {
            self.permits.forget_permits(current - n);
        }
    }

    fn emit(&self, job: &Job) {
        let _ = self.events.send(TransferEvent::Update(job.snapshot()));
    }

    fn set_state(&self, job: &Job, state: TransferState, error: Option<String>) {
        *job.state.lock().unwrap() = state;
        *job.error.lock().unwrap() = error;
        if state != TransferState::Running {
            *job.current.lock().unwrap() = None;
        }
        self.emit(job);
    }

    /// Met un transfert en file ; retourne son identifiant.
    pub fn enqueue(self: &Arc<Self>, spec: TransferSpec) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let job = Arc::new(Job {
            id,
            spec,
            ctl: AtomicU8::new(CTL_RUN),
            generation: AtomicU64::new(0),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
            files_done: AtomicU64::new(0),
            files_total: AtomicU64::new(0),
            base: AtomicU64::new(0),
            started: Mutex::new(None),
            last_emit: Mutex::new(None),
            state: Mutex::new(TransferState::Queued),
            error: Mutex::new(None),
            current: Mutex::new(None),
        });
        self.jobs.lock().unwrap().insert(id, job.clone());
        self.emit(&job);
        self.launch(job);
        id
    }

    fn launch(self: &Arc<Self>, job: Arc<Job>) {
        let generation = job.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let mgr = self.clone();
        tokio::spawn(async move { mgr.run(job, generation).await });
    }

    fn get(&self, id: u64) -> Option<Arc<Job>> {
        self.jobs.lock().unwrap().get(&id).cloned()
    }

    /// Pause : arrêt propre, `.part` conservé.
    pub fn pause(&self, id: u64) {
        let Some(job) = self.get(id) else { return };
        let state = *job.state.lock().unwrap();
        match state {
            TransferState::Running => job.ctl.store(CTL_PAUSE, Ordering::SeqCst),
            TransferState::Queued => {
                // Invalide l'exécution en attente de permis.
                job.generation.fetch_add(1, Ordering::SeqCst);
                self.set_state(&job, TransferState::Paused, None);
            }
            _ => {}
        }
    }

    /// Reprise d'un job en pause ou en erreur (repart du `.part`).
    pub fn resume(self: &Arc<Self>, id: u64) {
        let Some(job) = self.get(id) else { return };
        let state = *job.state.lock().unwrap();
        if matches!(state, TransferState::Paused | TransferState::Error) {
            job.ctl.store(CTL_RUN, Ordering::SeqCst);
            self.set_state(&job, TransferState::Queued, None);
            self.launch(job);
        }
    }

    /// Annulation : le `.part` du fichier en cours est supprimé.
    pub fn cancel(&self, id: u64) {
        let Some(job) = self.get(id) else { return };
        let state = *job.state.lock().unwrap();
        match state {
            TransferState::Running => job.ctl.store(CTL_CANCEL, Ordering::SeqCst),
            TransferState::Queued | TransferState::Paused | TransferState::Error => {
                job.generation.fetch_add(1, Ordering::SeqCst);
                self.set_state(&job, TransferState::Cancelled, None);
            }
            _ => {}
        }
    }

    pub fn list(&self) -> Vec<TransferInfo> {
        let mut v: Vec<TransferInfo> = self
            .jobs
            .lock()
            .unwrap()
            .values()
            .map(|j| j.snapshot())
            .collect();
        v.sort_by_key(|i| i.id);
        v
    }

    /// Retire de la liste les jobs terminés, annulés ou en erreur.
    pub fn clear_finished(&self) {
        self.jobs.lock().unwrap().retain(|_, j| {
            !matches!(
                *j.state.lock().unwrap(),
                TransferState::Done | TransferState::Cancelled | TransferState::Error
            )
        });
    }

    async fn run(self: Arc<Self>, job: Arc<Job>, generation: u64) {
        let Ok(_permit) = self.permits.clone().acquire_owned().await else {
            return;
        };
        if job.generation.load(Ordering::SeqCst) != generation {
            return; // pause/annulation/relance pendant l'attente
        }
        match job.ctl.load(Ordering::SeqCst) {
            CTL_CANCEL => return self.set_state(&job, TransferState::Cancelled, None),
            CTL_PAUSE => return self.set_state(&job, TransferState::Paused, None),
            _ => {}
        }
        *job.started.lock().unwrap() = Some(Instant::now());
        job.done.store(0, Ordering::SeqCst);
        job.base.store(0, Ordering::SeqCst);
        job.files_done.store(0, Ordering::SeqCst);
        self.set_state(&job, TransferState::Running, None);

        match self.execute(&job).await {
            Ok(Flow::Continue) => {
                job.done
                    .store(job.total.load(Ordering::Relaxed), Ordering::SeqCst);
                self.set_state(&job, TransferState::Done, None);
            }
            Ok(Flow::Paused) => self.set_state(&job, TransferState::Paused, None),
            Ok(Flow::Cancelled) => self.set_state(&job, TransferState::Cancelled, None),
            Err(e) => {
                tracing::warn!(job = job.id, "transfert en erreur : {e}");
                if is_fatal(&e) {
                    self.sessions.sftp_invalidate(&job.spec.session).await;
                }
                self.set_state(&job, TransferState::Error, Some(e.to_string()));
            }
        }
    }

    async fn execute(&self, job: &Arc<Job>) -> Result<Flow> {
        let sftp = self.sessions.sftp(&job.spec.session).await?;
        let items = match job.spec.direction {
            Direction::Download => plan_download(&sftp, &job.spec).await?,
            Direction::Upload => plan_upload(&sftp, &job.spec).await?,
        };
        let files: Vec<&Item> = items.iter().filter(|i| !i.is_dir).collect();
        job.total
            .store(files.iter().map(|i| i.size).sum(), Ordering::SeqCst);
        job.files_total.store(files.len() as u64, Ordering::SeqCst);
        self.emit(job);

        for item in &items {
            match job.ctl.load(Ordering::SeqCst) {
                CTL_PAUSE => return Ok(Flow::Paused),
                CTL_CANCEL => return Ok(Flow::Cancelled),
                _ => {}
            }
            if item.is_dir {
                ensure_dir(&sftp, job.spec.direction, &item.dst).await?;
                continue;
            }
            *job.current.lock().unwrap() = Some(base_name(&item.dst).to_string());
            let flow = match job.spec.direction {
                Direction::Download => self.download_file(&sftp, job, item).await?,
                Direction::Upload => self.upload_file(&sftp, job, item).await?,
            };
            match flow {
                Flow::Continue => {
                    job.files_done.fetch_add(1, Ordering::SeqCst);
                    self.emit(job);
                }
                other => return Ok(other),
            }
        }
        Ok(Flow::Continue)
    }

    fn progress(&self, job: &Job, n: u64) {
        job.done.fetch_add(n, Ordering::Relaxed);
        let mut last = job.last_emit.lock().unwrap();
        if last.is_none_or(|t| t.elapsed() >= EMIT_EVERY) {
            *last = Some(Instant::now());
            drop(last);
            self.emit(job);
        }
    }

    // ------------------------------------------------------------ téléchargement

    async fn download_file(&self, sftp: &SftpSession, job: &Arc<Job>, item: &Item) -> Result<Flow> {
        let dest = std::path::PathBuf::from(&item.dst);
        let part = part_path(&item.dst);
        if tokio::fs::metadata(&dest).await.is_ok() && job.spec.conflict == Conflict::Skip {
            job.done.fetch_add(item.size, Ordering::Relaxed);
            job.base.fetch_add(item.size, Ordering::Relaxed);
            return Ok(Flow::Continue);
        }
        let mut remote = sftp.open(item.src.as_str()).await.map_err(sftp_err)?;

        let mut offset = match tokio::fs::metadata(&part).await {
            Ok(m) if m.len() <= item.size => m.len(),
            _ => 0,
        };
        if offset > 0 && !tails_match_download(&mut remote, &part, offset).await {
            tracing::info!(file = %item.src, "reprise impossible (source modifiée ?) : redémarrage à 0");
            offset = 0;
        }
        remote
            .seek(SeekFrom::Start(offset))
            .await
            .map_err(io_sftp)?;
        let mut local = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(offset == 0)
            .open(&part)
            .await?;
        local.seek(SeekFrom::Start(offset)).await?;
        job.done.fetch_add(offset, Ordering::Relaxed);
        job.base.fetch_add(offset, Ordering::Relaxed);

        let mut buf = vec![0u8; CHUNK];
        loop {
            match job.ctl.load(Ordering::SeqCst) {
                CTL_PAUSE => {
                    local.flush().await?;
                    return Ok(Flow::Paused);
                }
                CTL_CANCEL => {
                    drop(local);
                    let _ = tokio::fs::remove_file(&part).await;
                    return Ok(Flow::Cancelled);
                }
                _ => {}
            }
            let n = remote.read(&mut buf).await.map_err(io_sftp)?;
            if n == 0 {
                break;
            }
            local.write_all(&buf[..n]).await?;
            self.progress(job, n as u64);
        }
        local.flush().await?;
        drop(local);
        let got = tokio::fs::metadata(&part).await?.len();
        if got != item.size {
            return Err(SshError::Sftp(format!(
                "taille inattendue pour {} : {got} octets reçus, {} attendus",
                item.src, item.size
            )));
        }
        tokio::fs::rename(&part, &dest).await?;
        if job.spec.preserve {
            apply_local_attrs(&dest, item).await;
        }
        Ok(Flow::Continue)
    }

    // ------------------------------------------------------------------ envoi

    async fn upload_file(&self, sftp: &SftpSession, job: &Arc<Job>, item: &Item) -> Result<Flow> {
        let part = part_path(&item.dst);
        let exists = sftp.try_exists(item.dst.as_str()).await.unwrap_or(false);
        if exists && job.spec.conflict == Conflict::Skip {
            job.done.fetch_add(item.size, Ordering::Relaxed);
            job.base.fetch_add(item.size, Ordering::Relaxed);
            return Ok(Flow::Continue);
        }
        let mut local = tokio::fs::File::open(&item.src).await?;

        let mut offset = match sftp.metadata(part.as_str()).await {
            Ok(m) if m.len() <= item.size => m.len(),
            _ => 0,
        };
        if offset > 0 && !tails_match_upload(sftp, &part, &mut local, offset).await {
            tracing::info!(file = %item.src, "reprise impossible (source modifiée ?) : redémarrage à 0");
            offset = 0;
        }
        let mut flags = OpenFlags::WRITE | OpenFlags::CREATE;
        if offset == 0 {
            flags |= OpenFlags::TRUNCATE;
        }
        let mut remote = sftp
            .open_with_flags(part.as_str(), flags)
            .await
            .map_err(sftp_err)?;
        remote
            .seek(SeekFrom::Start(offset))
            .await
            .map_err(io_sftp)?;
        local.seek(SeekFrom::Start(offset)).await?;
        job.done.fetch_add(offset, Ordering::Relaxed);
        job.base.fetch_add(offset, Ordering::Relaxed);

        let mut buf = vec![0u8; CHUNK];
        loop {
            match job.ctl.load(Ordering::SeqCst) {
                CTL_PAUSE => {
                    let _ = remote.shutdown().await;
                    return Ok(Flow::Paused);
                }
                CTL_CANCEL => {
                    let _ = remote.shutdown().await;
                    let _ = sftp.remove_file(part.as_str()).await;
                    return Ok(Flow::Cancelled);
                }
                _ => {}
            }
            let n = local.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            remote.write_all(&buf[..n]).await.map_err(io_sftp)?;
            self.progress(job, n as u64);
        }
        remote.shutdown().await.map_err(io_sftp)?;
        drop(remote);

        let got = sftp.metadata(part.as_str()).await.map_err(sftp_err)?.len();
        if got != item.size {
            return Err(SshError::Sftp(format!(
                "taille inattendue pour {} : {got} octets écrits, {} attendus",
                item.dst, item.size
            )));
        }
        if exists {
            // SFTP v3 : rename n'écrase pas.
            sftp.remove_file(item.dst.as_str())
                .await
                .map_err(sftp_err)?;
        }
        sftp.rename(part.as_str(), item.dst.as_str())
            .await
            .map_err(sftp_err)?;
        if job.spec.preserve {
            let attrs = FileAttributes {
                permissions: Some(item.mode),
                atime: Some(item.mtime),
                mtime: Some(item.mtime),
                ..FileAttributes::empty()
            };
            // Best effort : un serveur peut refuser setstat sans invalider le transfert.
            if let Err(e) = sftp.set_metadata(item.dst.as_str(), attrs).await {
                tracing::debug!("préservation des attributs refusée : {e}");
            }
        }
        Ok(Flow::Continue)
    }
}

fn sftp_err(e: russh_sftp::client::error::Error) -> SshError {
    SshError::Sftp(e.to_string())
}

fn io_sftp(e: std::io::Error) -> SshError {
    SshError::Sftp(format!("io error: {e}"))
}

pub fn part_path(dest: &str) -> String {
    format!("{dest}.part")
}

// ------------------------------------------------------------------ reprise

/// Compare la fin du `.part` local avec le même intervalle côté distant.
async fn tails_match_download(
    remote: &mut russh_sftp::client::fs::File,
    part: &str,
    offset: u64,
) -> bool {
    let len = offset.min(TAIL_CHECK);
    let start = offset - len;
    let Ok(mut local) = tokio::fs::File::open(part).await else {
        return false;
    };
    let mut a = vec![0u8; len as usize];
    let mut b = vec![0u8; len as usize];
    let ok = async {
        local.seek(SeekFrom::Start(start)).await.ok()?;
        local.read_exact(&mut a).await.ok()?;
        remote.seek(SeekFrom::Start(start)).await.ok()?;
        remote.read_exact(&mut b).await.ok()?;
        Some(())
    }
    .await;
    ok.is_some() && a == b
}

/// Compare la fin du `.part` distant avec le même intervalle du fichier local.
async fn tails_match_upload(
    sftp: &SftpSession,
    part: &str,
    local: &mut tokio::fs::File,
    offset: u64,
) -> bool {
    let len = offset.min(TAIL_CHECK);
    let start = offset - len;
    let Ok(mut remote) = sftp.open(part).await else {
        return false;
    };
    let mut a = vec![0u8; len as usize];
    let mut b = vec![0u8; len as usize];
    let ok = async {
        local.seek(SeekFrom::Start(start)).await.ok()?;
        local.read_exact(&mut a).await.ok()?;
        remote.seek(SeekFrom::Start(start)).await.ok()?;
        remote.read_exact(&mut b).await.ok()?;
        Some(())
    }
    .await;
    ok.is_some() && a == b
}

// ------------------------------------------------------------------ attributs

async fn apply_local_attrs(dest: &std::path::Path, item: &Item) {
    use std::os::unix::fs::PermissionsExt;
    let _ = tokio::fs::set_permissions(dest, std::fs::Permissions::from_mode(item.mode)).await;
    let mtime = std::time::UNIX_EPOCH + Duration::from_secs(u64::from(item.mtime));
    let dest = dest.to_path_buf();
    let _ = tokio::task::spawn_blocking(move || {
        std::fs::File::options()
            .write(true)
            .open(&dest)
            .and_then(|f| f.set_modified(mtime))
    })
    .await;
}

async fn ensure_dir(sftp: &SftpSession, dir: Direction, path: &str) -> Result<()> {
    match dir {
        Direction::Download => {
            tokio::fs::create_dir_all(path).await?;
        }
        Direction::Upload => {
            if !sftp.try_exists(path).await.unwrap_or(false) {
                sftp.create_dir(path).await.map_err(sftp_err)?;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ plans

fn unix_secs(t: std::io::Result<std::time::SystemTime>) -> u32 {
    t.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

async fn plan_upload(_sftp: &SftpSession, spec: &TransferSpec) -> Result<Vec<Item>> {
    use std::os::unix::fs::PermissionsExt;
    let root = std::path::PathBuf::from(&spec.source);
    let meta = tokio::fs::metadata(&root).await?; // suit un lien en racine
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| SshError::Sftp(format!("chemin local invalide : {}", spec.source)))?;
    let dst_root = join(&spec.dest_dir, &name);
    let mut items = Vec::new();
    if !meta.is_dir() {
        items.push(Item {
            src: spec.source.clone(),
            dst: dst_root,
            size: meta.len(),
            mode: meta.permissions().mode() & 0o7777,
            mtime: unix_secs(meta.modified()),
            is_dir: false,
        });
        return Ok(items);
    }
    let mut stack = vec![(root, dst_root, 0usize)];
    while let Some((local_dir, remote_dir, depth)) = stack.pop() {
        items.push(Item {
            src: local_dir.to_string_lossy().into_owned(),
            dst: remote_dir.clone(),
            size: 0,
            mode: 0o755,
            mtime: 0,
            is_dir: true,
        });
        if depth >= MAX_DEPTH {
            tracing::warn!(dir = %local_dir.display(), "profondeur max atteinte, sous-arbre ignoré");
            continue;
        }
        let mut rd = tokio::fs::read_dir(&local_dir).await?;
        let mut subdirs = Vec::new();
        while let Some(entry) = rd.next_entry().await? {
            let path = entry.path();
            let ft = entry.file_type().await?;
            let m = if ft.is_symlink() {
                if spec.symlinks == SymlinkPolicy::Skip {
                    continue;
                }
                match tokio::fs::metadata(&path).await {
                    Ok(m) => m,
                    Err(_) => continue, // lien cassé
                }
            } else {
                entry.metadata().await?
            };
            let rname = join(&remote_dir, &entry.file_name().to_string_lossy());
            if m.is_dir() {
                subdirs.push((path, rname, depth + 1));
            } else if m.is_file() {
                items.push(Item {
                    src: path.to_string_lossy().into_owned(),
                    dst: rname,
                    size: m.len(),
                    mode: m.permissions().mode() & 0o7777,
                    mtime: unix_secs(m.modified()),
                    is_dir: false,
                });
            }
        }
        // Les sous-dossiers sont traités après les fichiers du dossier courant.
        stack.extend(subdirs.into_iter().rev());
    }
    Ok(items)
}

async fn plan_download(sftp: &SftpSession, spec: &TransferSpec) -> Result<Vec<Item>> {
    let local_root = std::path::Path::new(&spec.dest_dir).join(base_name(&spec.source));
    let local_root = local_root.to_string_lossy().into_owned();
    let meta = sftp
        .metadata(spec.source.as_str())
        .await
        .map_err(sftp_err)?; // suit un lien en racine
    let mut items = Vec::new();
    if !meta.file_type().is_dir() {
        items.push(Item {
            src: spec.source.clone(),
            dst: local_root,
            size: meta.len(),
            mode: meta.permissions.unwrap_or(0o644) & 0o7777,
            mtime: meta.mtime.unwrap_or(0),
            is_dir: false,
        });
        return Ok(items);
    }
    walk_remote(sftp, spec, &spec.source, &local_root, 0, &mut items).await?;
    Ok(items)
}

fn walk_remote<'a>(
    sftp: &'a SftpSession,
    spec: &'a TransferSpec,
    remote_dir: &'a str,
    local_dir: &'a str,
    depth: usize,
    items: &'a mut Vec<Item>,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        items.push(Item {
            src: remote_dir.to_string(),
            dst: local_dir.to_string(),
            size: 0,
            mode: 0o755,
            mtime: 0,
            is_dir: true,
        });
        if depth >= MAX_DEPTH {
            tracing::warn!(
                dir = remote_dir,
                "profondeur max atteinte, sous-arbre ignoré"
            );
            return Ok(());
        }
        let read = sftp.read_dir(remote_dir).await.map_err(sftp_err)?;
        let mut subdirs = Vec::new();
        for entry in read {
            let rpath = join(remote_dir, &entry.file_name());
            let lpath = std::path::Path::new(local_dir)
                .join(entry.file_name())
                .to_string_lossy()
                .into_owned();
            let mut attrs = entry.metadata();
            if entry.file_type().is_symlink() {
                if spec.symlinks == SymlinkPolicy::Skip {
                    continue;
                }
                match sftp.metadata(rpath.as_str()).await {
                    Ok(m) => attrs = m,
                    Err(_) => continue, // lien cassé
                }
            }
            if attrs.file_type().is_dir() {
                subdirs.push((rpath, lpath));
            } else if attrs.file_type().is_file() {
                items.push(Item {
                    src: rpath,
                    dst: lpath,
                    size: attrs.len(),
                    mode: attrs.permissions.unwrap_or(0o644) & 0o7777,
                    mtime: attrs.mtime.unwrap_or(0),
                    is_dir: false,
                });
            }
        }
        for (r, l) in subdirs {
            walk_remote(sftp, spec, &r, &l, depth + 1, items).await?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_suffix() {
        assert_eq!(part_path("/a/b.bin"), "/a/b.bin.part");
    }

    #[test]
    fn spec_defaults_are_safe() {
        let spec: TransferSpec = serde_json::from_str(
            r#"{"session":"s","direction":"download","source":"/a","dest_dir":"/tmp"}"#,
        )
        .unwrap();
        assert_eq!(spec.conflict, Conflict::Skip);
        assert_eq!(spec.symlinks, SymlinkPolicy::Skip);
        assert!(spec.preserve);
    }
}
