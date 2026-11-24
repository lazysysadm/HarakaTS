//! Édition à distance (jalon M6) : téléchargement vers un répertoire temporaire
//! privé, suivi de l'état distant (mtime + taille + SHA-256), envoi atomique,
//! détection de conflit, éditeur externe surveillé.
//!
//! Garanties :
//! - répertoire temporaire `0700`, fichier `0600`, supprimés à la fermeture du
//!   document (et purgés au démarrage de l'application, cf. `DataDir::purge_tmp`) ;
//! - **jamais d'écrasement silencieux** : à l'enregistrement, l'état distant est
//!   comparé à celui du dernier téléchargement/envoi ; s'il a changé (mtime/taille
//!   puis contenu SHA-256), c'est un conflit, résolu explicitement par l'utilisateur ;
//! - envoi en deux temps : fichier temporaire distant dans le MÊME répertoire, droits
//!   copiés, puis remplacement. SFTP v3 ne permet pas d'écraser par `rename` : la cible
//!   est d'abord renommée en sauvegarde puis restaurée en cas d'échec — pas de fichier
//!   partiel visible sous son nom, mais une courte fenêtre sans fichier pendant le
//!   remplacement (documentée) ;
//! - fins de ligne et octets préservés tels quels (le contenu est manipulé en octets).

use std::collections::HashMap;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileAttributes;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, mpsc};

use crate::error::{Result, SshError};
use crate::session::SessionManager;
use crate::sftp::{base_name, is_fatal, join};

/// Taille maximale éditable (au-delà : téléchargement refusé pour l'édition).
pub const MAX_EDIT_BYTES: u64 = 20 * 1024 * 1024;
/// Délai de stabilisation avant l'envoi automatique d'un fichier modifié par
/// l'éditeur externe (les éditeurs écrivent souvent en plusieurs étapes).
const DEBOUNCE: Duration = Duration::from_millis(400);

type Sha = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    Lf,
    Crlf,
    Mixed,
    None,
}

/// Fins de ligne dominantes d'un contenu.
pub fn detect_line_ending(bytes: &[u8]) -> LineEnding {
    let (mut crlf, mut lf) = (0usize, 0usize);
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            if i > 0 && bytes[i - 1] == b'\r' {
                crlf += 1;
            } else {
                lf += 1;
            }
        }
    }
    match (crlf, lf) {
        (0, 0) => LineEnding::None,
        (0, _) => LineEnding::Lf,
        (_, 0) => LineEnding::Crlf,
        _ => LineEnding::Mixed,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EditDocInfo {
    pub id: String,
    pub session: String,
    pub remote_path: String,
    pub name: String,
    pub size: u64,
    pub mtime: Option<u32>,
    pub mode: Option<u32>,
    pub line_ending: LineEnding,
    pub utf8: bool,
    pub external: bool,
}

/// État du fichier distant au moment d'un conflit.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteState {
    pub exists: bool,
    pub size: u64,
    pub mtime: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum SaveOutcome {
    Saved { info: EditDocInfo },
    Conflict { remote: RemoteState },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum EditEvent {
    Saved { id: String, info: EditDocInfo },
    Conflict { id: String, remote: RemoteState },
    Error { id: String, message: String },
}

/// Référence synchronisée avec le distant (dernier téléchargement / envoi).
#[derive(Clone)]
struct Base {
    mtime: Option<u32>,
    size: u64,
    sha: Sha,
}

struct DocState {
    base: Base,
    mode: Option<u32>,
    line_ending: LineEnding,
    utf8: bool,
    external: bool,
    /// Tâche de surveillance (éditeur externe) : arrêtée à la fermeture.
    watcher: Option<tokio::task::JoinHandle<()>>,
    /// Garde du watcher `notify` (arrêt à la destruction).
    _guard: Option<notify::RecommendedWatcher>,
}

struct Doc {
    id: String,
    session: String,
    remote_path: String,
    name: String,
    dir: PathBuf,
    file: PathBuf,
    state: Mutex<DocState>,
    /// Sérialise les enregistrements d'un même document.
    save_lock: Mutex<()>,
}

pub struct EditManager {
    sessions: Arc<SessionManager>,
    tmp_root: PathBuf,
    docs: Mutex<HashMap<String, Arc<Doc>>>,
    events: mpsc::UnboundedSender<EditEvent>,
}

fn sftp_err(e: russh_sftp::client::error::Error) -> SshError {
    SshError::Sftp(e.to_string())
}

fn sha_of(bytes: &[u8]) -> Sha {
    Sha256::digest(bytes).into()
}

impl Doc {
    async fn info(&self) -> EditDocInfo {
        let st = self.state.lock().await;
        self.info_with(&st)
    }

    fn info_with(&self, st: &DocState) -> EditDocInfo {
        EditDocInfo {
            id: self.id.clone(),
            session: self.session.clone(),
            remote_path: self.remote_path.clone(),
            name: self.name.clone(),
            size: st.base.size,
            mtime: st.base.mtime,
            mode: st.mode,
            line_ending: st.line_ending,
            utf8: st.utf8,
            external: st.external,
        }
    }
}

/// Écrit le fichier temporaire local (0600) de façon atomique.
fn write_local(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = file.with_extension("haraka-write");
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
    }
    std::fs::rename(&tmp, file)
}

impl EditManager {
    pub fn new(
        sessions: Arc<SessionManager>,
        tmp_root: PathBuf,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<EditEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                sessions,
                tmp_root,
                docs: Mutex::new(HashMap::new()),
                events: tx,
            }),
            rx,
        )
    }

    async fn doc(&self, id: &str) -> Result<Arc<Doc>> {
        self.docs
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| SshError::NotFound(id.to_string()))
    }

    async fn sftp_of(&self, doc: &Doc) -> Result<Arc<SftpSession>> {
        self.sessions.sftp(&doc.session).await
    }

    /// Ouvre un fichier distant : téléchargement vers le répertoire temporaire privé.
    /// Retourne l'info du document et les octets (le contenu n'est pas relu du disque).
    pub async fn open(&self, session: &str, remote_path: &str) -> Result<(EditDocInfo, Vec<u8>)> {
        let sftp = self.sessions.sftp(session).await?;
        let meta = sftp.metadata(remote_path).await.map_err(sftp_err)?;
        if !meta.file_type().is_file() {
            return Err(SshError::Sftp(format!(
                "n'est pas un fichier régulier : {remote_path}"
            )));
        }
        if meta.len() > MAX_EDIT_BYTES {
            return Err(SshError::Sftp(format!(
                "fichier trop volumineux pour l'édition ({} octets, max {MAX_EDIT_BYTES})",
                meta.len()
            )));
        }
        let bytes = sftp.read(remote_path).await.map_err(sftp_err)?;
        let id = uuid::Uuid::new_v4().to_string();
        let name = base_name(remote_path).to_string();
        let dir = self.tmp_root.join(&id);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
        // Le répertoire parent peut avoir été créé avec d'autres droits : on force.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        let file = dir.join(&name);
        write_local(&file, &bytes)?;

        let state = DocState {
            base: Base {
                mtime: meta.mtime,
                size: bytes.len() as u64,
                sha: sha_of(&bytes),
            },
            mode: meta.permissions.map(|p| p & 0o7777),
            line_ending: detect_line_ending(&bytes),
            utf8: std::str::from_utf8(&bytes).is_ok(),
            external: false,
            watcher: None,
            _guard: None,
        };
        let doc = Arc::new(Doc {
            id: id.clone(),
            session: session.to_string(),
            remote_path: remote_path.to_string(),
            name,
            dir,
            file,
            state: Mutex::new(state),
            save_lock: Mutex::new(()),
        });
        let info = doc.info().await;
        self.docs.lock().await.insert(id, doc);
        Ok((info, bytes))
    }

    /// Nouveau téléchargement : abandonne les modifications locales et repart de l'état distant.
    pub async fn reload(&self, id: &str) -> Result<(EditDocInfo, Vec<u8>)> {
        let doc = self.doc(id).await?;
        let _g = doc.save_lock.lock().await;
        let sftp = self.sftp_of(&doc).await?;
        let meta = sftp
            .metadata(doc.remote_path.as_str())
            .await
            .map_err(sftp_err)?;
        if meta.len() > MAX_EDIT_BYTES {
            return Err(SshError::Sftp(
                "fichier trop volumineux pour l'édition".into(),
            ));
        }
        let bytes = sftp
            .read(doc.remote_path.as_str())
            .await
            .map_err(sftp_err)?;
        write_local(&doc.file, &bytes)?;
        let mut st = doc.state.lock().await;
        st.base = Base {
            mtime: meta.mtime,
            size: bytes.len() as u64,
            sha: sha_of(&bytes),
        };
        st.mode = meta.permissions.map(|p| p & 0o7777);
        st.line_ending = detect_line_ending(&bytes);
        st.utf8 = std::str::from_utf8(&bytes).is_ok();
        let info = doc.info_with(&st);
        Ok((info, bytes))
    }

    /// Contenu distant actuel (comparaison de conflit). `None` si le fichier n'existe plus.
    pub async fn remote_bytes(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let doc = self.doc(id).await?;
        let sftp = self.sftp_of(&doc).await?;
        match sftp.metadata(doc.remote_path.as_str()).await {
            Ok(m) if m.len() > MAX_EDIT_BYTES => {
                Err(SshError::Sftp("fichier distant trop volumineux".into()))
            }
            Ok(_) => Ok(Some(
                sftp.read(doc.remote_path.as_str())
                    .await
                    .map_err(sftp_err)?,
            )),
            Err(_) => Ok(None),
        }
    }

    /// Chemin du fichier temporaire local d'un document.
    pub async fn local_path(&self, id: &str) -> Result<PathBuf> {
        Ok(self.doc(id).await?.file.clone())
    }

    /// Contenu actuel du fichier temporaire local (documents édités en externe).
    pub async fn local_bytes(&self, id: &str) -> Result<Vec<u8>> {
        let doc = self.doc(id).await?;
        Ok(tokio::fs::read(&doc.file).await?)
    }

    /// Enregistre `bytes` sur le distant. `overwrite` = l'utilisateur a choisi d'écraser.
    pub async fn save_bytes(
        &self,
        id: &str,
        bytes: Vec<u8>,
        overwrite: bool,
    ) -> Result<SaveOutcome> {
        let doc = self.doc(id).await?;
        let _g = doc.save_lock.lock().await;
        let sftp = self.sftp_of(&doc).await?;
        let path = doc.remote_path.as_str();

        // 1. Conflit ? (sauf écrasement explicite)
        let remote_meta = sftp.metadata(path).await.ok();
        if !overwrite {
            let base = doc.state.lock().await.base.clone();
            match &remote_meta {
                None => {
                    return Ok(SaveOutcome::Conflict {
                        remote: RemoteState {
                            exists: false,
                            size: 0,
                            mtime: None,
                        },
                    });
                }
                Some(m) if m.mtime != base.mtime || m.len() != base.size => {
                    // mtime/taille changés : seul le contenu tranche (un `touch` n'est pas un conflit).
                    let current = sftp.read(path).await.map_err(sftp_err)?;
                    if sha_of(&current) != base.sha {
                        return Ok(SaveOutcome::Conflict {
                            remote: RemoteState {
                                exists: true,
                                size: m.len(),
                                mtime: m.mtime,
                            },
                        });
                    }
                }
                Some(_) => {}
            }
        }

        // 2. Envoi : temporaire distant (même répertoire), droits, remplacement.
        let mode = {
            let st = doc.state.lock().await;
            remote_meta
                .as_ref()
                .and_then(|m| m.permissions.map(|p| p & 0o7777))
                .or(st.mode)
        };
        let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let dir = if dir.is_empty() { "/" } else { dir };
        let short = &doc.id[..8];
        let tmp_remote = join(dir, &format!(".{}.haraka-{short}.tmp", doc.name));
        let bak_remote = join(dir, &format!(".{}.haraka-{short}.bak", doc.name));

        let upload = async {
            let mut f = sftp.create(tmp_remote.as_str()).await.map_err(sftp_err)?;
            f.write_all(&bytes)
                .await
                .map_err(|e| SshError::Sftp(format!("io error: {e}")))?;
            f.shutdown()
                .await
                .map_err(|e| SshError::Sftp(format!("io error: {e}")))?;
            drop(f);
            if let Some(mode) = mode {
                let attrs = FileAttributes {
                    permissions: Some(mode),
                    ..FileAttributes::empty()
                };
                // Meilleur effort : un serveur peut refuser setstat.
                let _ = sftp.set_metadata(tmp_remote.as_str(), attrs).await;
            }
            Ok::<(), SshError>(())
        };
        if let Err(e) = upload.await {
            let _ = sftp.remove_file(tmp_remote.as_str()).await;
            return Err(e);
        }

        let replaced = if remote_meta.is_some() {
            // SFTP v3 : `rename` n'écrase pas → cible en sauvegarde, puis remplacement.
            match sftp.rename(path, bak_remote.as_str()).await {
                Ok(()) => match sftp.rename(tmp_remote.as_str(), path).await {
                    Ok(()) => {
                        let _ = sftp.remove_file(bak_remote.as_str()).await;
                        Ok(())
                    }
                    Err(e) => {
                        // Restaure l'original : jamais de fichier perdu.
                        let _ = sftp.rename(bak_remote.as_str(), path).await;
                        Err(e)
                    }
                },
                Err(e) => Err(e),
            }
        } else {
            sftp.rename(tmp_remote.as_str(), path).await
        };
        if let Err(e) = replaced {
            let _ = sftp.remove_file(tmp_remote.as_str()).await;
            return Err(sftp_err(e));
        }

        // 3. Nouvelle référence + fichier temporaire local à jour.
        let new_meta = sftp.metadata(path).await.ok();
        write_local(&doc.file, &bytes)?;
        let mut st = doc.state.lock().await;
        st.base = Base {
            mtime: new_meta.as_ref().and_then(|m| m.mtime),
            size: bytes.len() as u64,
            sha: sha_of(&bytes),
        };
        st.mode = mode;
        st.line_ending = detect_line_ending(&bytes);
        st.utf8 = std::str::from_utf8(&bytes).is_ok();
        Ok(SaveOutcome::Saved {
            info: doc.info_with(&st),
        })
    }

    /// Envoie le contenu ACTUEL du fichier temporaire (résolution d'un conflit
    /// d'un document édité en externe).
    pub async fn save_local(&self, id: &str, overwrite: bool) -> Result<SaveOutcome> {
        let bytes = self.local_bytes(id).await?;
        self.save_bytes(id, bytes, overwrite).await
    }

    /// Lance l'éditeur externe sur le fichier temporaire et surveille ses
    /// enregistrements (envoi automatique, débounce). `argv` : programme + arguments
    /// (déjà construits, jamais via un shell).
    pub async fn open_external(self: &Arc<Self>, id: &str, argv: Vec<String>) -> Result<()> {
        let doc = self.doc(id).await?;
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| SshError::Sftp("commande d'éditeur vide".into()))?;

        // Surveillance AVANT le lancement : aucune écriture ne peut être manquée.
        {
            let mut st = doc.state.lock().await;
            if st.watcher.is_none() {
                let (ntx, nrx) = mpsc::unbounded_channel::<()>();
                let file = doc.file.clone();
                let dir = doc.dir.clone();
                let mut watcher = notify::recommended_watcher(
                    move |res: std::result::Result<notify::Event, notify::Error>| {
                        if let Ok(ev) = res
                            && ev.paths.iter().any(|p| {
                                // Les éditeurs écrivent souvent via un fichier voisin + rename.
                                p == &file || p.parent() == Some(dir.as_path())
                            })
                        {
                            let _ = ntx.send(());
                        }
                    },
                )
                .map_err(|e| SshError::Sftp(format!("surveillance impossible : {e}")))?;
                notify::Watcher::watch(&mut watcher, &doc.dir, notify::RecursiveMode::NonRecursive)
                    .map_err(|e| SshError::Sftp(format!("surveillance impossible : {e}")))?;
                st._guard = Some(watcher);
                let mgr = self.clone();
                let doc2 = doc.clone();
                st.watcher = Some(tokio::spawn(async move { mgr.watch_loop(doc2, nrx).await }));
            }
            st.external = true;
        }

        std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| {
                SshError::Sftp(format!(
                    "éditeur externe impossible à lancer ({program}) : {e}"
                ))
            })?;
        Ok(())
    }

    async fn watch_loop(self: Arc<Self>, doc: Arc<Doc>, mut rx: mpsc::UnboundedReceiver<()>) {
        while rx.recv().await.is_some() {
            // Débounce : on attend que les écritures cessent.
            loop {
                match tokio::time::timeout(DEBOUNCE, rx.recv()).await {
                    Ok(Some(())) => continue,
                    Ok(None) => return,
                    Err(_) => break,
                }
            }
            let Ok(bytes) = tokio::fs::read(&doc.file).await else {
                continue; // fichier remplacé en cours d'écriture
            };
            if sha_of(&bytes) == doc.state.lock().await.base.sha {
                continue; // aucune modification réelle (écho de notre propre écriture)
            }
            let event = match self.save_bytes(&doc.id, bytes, false).await {
                Ok(SaveOutcome::Saved { info }) => EditEvent::Saved {
                    id: doc.id.clone(),
                    info,
                },
                Ok(SaveOutcome::Conflict { remote }) => EditEvent::Conflict {
                    id: doc.id.clone(),
                    remote,
                },
                Err(e) => {
                    if is_fatal(&e) {
                        self.sessions.sftp_invalidate(&doc.session).await;
                    }
                    EditEvent::Error {
                        id: doc.id.clone(),
                        message: e.to_string(),
                    }
                }
            };
            let _ = self.events.send(event);
        }
    }

    pub async fn list(&self) -> Vec<EditDocInfo> {
        let docs: Vec<Arc<Doc>> = self.docs.lock().await.values().cloned().collect();
        let mut out = Vec::new();
        for d in docs {
            out.push(d.info().await);
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Ferme un document : arrêt de la surveillance, suppression des temporaires.
    pub async fn close(&self, id: &str) -> Result<()> {
        let doc = self
            .docs
            .lock()
            .await
            .remove(id)
            .ok_or_else(|| SshError::NotFound(id.to_string()))?;
        {
            let mut st = doc.state.lock().await;
            st._guard = None;
            if let Some(h) = st.watcher.take() {
                h.abort();
            }
        }
        let _ = haraka_core::store::secure_remove_all(&doc.dir);
        Ok(())
    }

    /// Ferme tout (verrouillage du coffre, fermeture de session).
    pub async fn close_all(&self) {
        let ids: Vec<String> = self.docs.lock().await.keys().cloned().collect();
        for id in ids {
            let _ = self.close(&id).await;
        }
    }
}

/// Construit la ligne de commande de l'éditeur externe SANS shell : découpage façon
/// shell (espaces, guillemets simples/doubles, antislash), puis `{file}` remplacé
/// dans les arguments, ou le chemin ajouté en dernier argument s'il n'y figure pas.
/// Commande vide → `xdg-open <fichier>` (application par défaut du système).
pub fn build_editor_argv(command: Option<&str>, file: &str) -> Result<Vec<String>> {
    let command = command.map(str::trim).filter(|c| !c.is_empty());
    let Some(command) = command else {
        return Ok(vec!["xdg-open".into(), file.into()]);
    };
    let mut args: Vec<String> = split_command(command)?;
    if args.is_empty() {
        return Ok(vec!["xdg-open".into(), file.into()]);
    }
    let mut replaced = false;
    for a in &mut args {
        if a.contains("{file}") {
            *a = a.replace("{file}", file);
            replaced = true;
        }
    }
    if !replaced {
        args.push(file.into());
    }
    Ok(args)
}

fn split_command(s: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_token = true;
            }
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    in_token = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if in_token {
                    out.push(std::mem::take(&mut cur));
                    in_token = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_token = true;
            }
        }
    }
    if quote.is_some() {
        return Err(SshError::Sftp(
            "guillemet non fermé dans la commande de l'éditeur".into(),
        ));
    }
    if in_token {
        out.push(cur);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings() {
        assert_eq!(detect_line_ending(b"a\nb\n"), LineEnding::Lf);
        assert_eq!(detect_line_ending(b"a\r\nb\r\n"), LineEnding::Crlf);
        assert_eq!(detect_line_ending(b"a\r\nb\n"), LineEnding::Mixed);
        assert_eq!(detect_line_ending(b"abc"), LineEnding::None);
        assert_eq!(detect_line_ending(b""), LineEnding::None);
    }

    #[test]
    fn editor_argv_no_shell() {
        assert_eq!(
            build_editor_argv(None, "/t/f.txt").unwrap(),
            vec!["xdg-open", "/t/f.txt"]
        );
        assert_eq!(
            build_editor_argv(Some("code --wait"), "/t/f.txt").unwrap(),
            vec!["code", "--wait", "/t/f.txt"]
        );
        assert_eq!(
            build_editor_argv(
                Some("emacsclient -c --eval '(find-file \"{file}\")'"),
                "/t/a b.txt"
            )
            .unwrap(),
            vec!["emacsclient", "-c", "--eval", "(find-file \"/t/a b.txt\")"]
        );
        assert_eq!(
            build_editor_argv(Some("  "), "/t/f").unwrap(),
            vec!["xdg-open", "/t/f"]
        );
        // Pas de shell : un `;` reste un argument littéral.
        assert_eq!(
            build_editor_argv(Some("vim ; rm -rf /"), "/t/f").unwrap(),
            vec!["vim", ";", "rm", "-rf", "/", "/t/f"]
        );
        assert!(build_editor_argv(Some("vim 'oops"), "/t/f").is_err());
    }

    #[test]
    fn sha_is_stable() {
        assert_eq!(sha_of(b"abc"), sha_of(b"abc"));
        assert_ne!(sha_of(b"abc"), sha_of(b"abd"));
    }
}
