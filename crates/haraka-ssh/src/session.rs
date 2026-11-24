//! Sessions SSH (jalon M3) : connexion chaînée (jump hosts), authentification
//! (mot de passe, clé, agent, keyboard-interactive), vérification des host keys
//! (TOFU + alerte bloquante si changement), shell PTY, keepalive, reconnexion.
//!
//! APIs russh 0.65.2 vérifiées sur docs.rs (voir ARCHITECTURE.md §7).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::client::AgentClient;
use russh::keys::{
    Algorithm, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, decode_secret_key,
};
use russh::{Channel, ChannelId, ChannelWriteHalf, Disconnect};
use russh_sftp::client::SftpSession;
use secrecy::{ExposeSecret, SecretString};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, mpsc, oneshot};
use uuid::Uuid;

use haraka_core::knownhosts::{self, KnownHostStatus};

use crate::error::{Result, SshError};

type EventTx = mpsc::UnboundedSender<SessionEvent>;
type OutputTx = mpsc::UnboundedSender<Vec<u8>>;
/// Sortie partageable : permet le ré-attachement (onglet détaché) en
/// remplaçant le canal de sortie sans recréer la session.
type SharedOutput = Arc<Mutex<OutputTx>>;

trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}
type BoxStream = Box<dyn AsyncStream>;

/// Matériau d'authentification résolu par le backend (jamais par l'UI).
#[derive(Clone)]
pub enum AuthMaterial {
    Password(SecretString),
    Key {
        pem: SecretString,
        passphrase: Option<SecretString>,
    },
    Agent,
    KeyboardInteractive {
        /// Mot de passe du credential, utilisé pour pré-remplir un prompt
        /// « password » unique (sinon les prompts vont à l'UI).
        prefill: Option<SecretString>,
    },
}

#[derive(Clone)]
pub struct HopConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub materials: Vec<AuthMaterial>,
}

#[derive(Clone)]
pub struct ReconnectPolicy {
    pub enabled: bool,
    pub max_attempts: u32,
    pub backoff_max_s: u32,
}

#[derive(Clone)]
pub struct OpenRequest {
    pub session_id: String,
    pub hops: Vec<HopConfig>,
    pub target: HopConfig,
    pub cols: u32,
    pub rows: u32,
    pub keepalive_s: u32,
    pub reconnect: ReconnectPolicy,
    pub known_hosts: PathBuf,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PromptDto {
    pub text: String,
    pub echo: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    State {
        session: String,
        state: String,
        detail: Option<String>,
    },
    Banner {
        session: String,
        text: String,
    },
    AuthPrompt {
        session: String,
        prompt_id: String,
        name: String,
        instructions: String,
        prompts: Vec<PromptDto>,
    },
    HostKeyPrompt {
        session: String,
        prompt_id: String,
        host: String,
        port: u16,
        kind: String,
        fingerprint: String,
        old_fingerprint: Option<String>,
    },
    ExitStatus {
        session: String,
        code: u32,
    },
    Closed {
        session: String,
        reason: String,
    },
}

pub struct HostKeyDecision {
    pub accept: bool,
    pub remember: bool,
}

#[derive(Default)]
struct Pending {
    auth: HashMap<String, oneshot::Sender<Vec<String>>>,
    hostkey: HashMap<String, oneshot::Sender<HostKeyDecision>>,
}

struct SessionEntry {
    // Arc : `sftp()` ouvre un second canal (sous-système sftp) sur la même
    // connexion, hors du verrou global.
    handle: Arc<Handle<SshHandler>>,
    /// Session SFTP ouverte à la demande (une par session SSH, pas de seconde
    /// authentification). Mutex tokio : ouverture mono-vol.
    sftp: Arc<Mutex<Option<Arc<SftpSession>>>>,
    #[allow(dead_code)]
    hops: Vec<Handle<SshHandler>>,
    // Arc : write/resize clonent le handle sous le verrou global puis
    // attendent le réseau HORS verrou (sinon une session bloquée gèle
    // toutes les autres, avec risque d'interblocage avec disconnected()).
    /// Moitié écriture du canal shell. La moitié lecture est drainée par une
    /// tâche dédiée (voir `run_session`) : sans cela le tampon interne de russh
    /// (100 messages) se remplit et la boucle de session SSH se bloque.
    channel: Arc<ChannelWriteHalf<client::Msg>>,
    request: OpenRequest,
    output: SharedOutput,
    closed: Arc<AtomicBool>,
}

#[derive(Default)]
struct ManagerInner {
    sessions: HashMap<String, SessionEntry>,
    pending: Pending,
    states: HashMap<String, String>,
    /// Identification SSH annoncée par le serveur (`SSH-2.0-OpenSSH_9.6`, `SSH-2.0-ROSSSH`…).
    idents: HashMap<String, String>,
    /// Tunnels distants (-R) : connexions `forwarded-tcpip` -> tunnel (M7).
    forwards: crate::tunnel::ForwardRegistry,
}

pub struct SessionManager {
    inner: Arc<Mutex<ManagerInner>>,
    events: EventTx,
}

impl SessionManager {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<SessionEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                inner: Arc::new(Mutex::new(ManagerInner::default())),
                events: tx,
            },
            rx,
        )
    }

    fn emit(&self, ev: SessionEvent) {
        let _ = self.events.send(ev);
    }

    /// Ouvre une session (asynchrone : les événements suivent la progression).
    pub async fn open(&self, req: OpenRequest, output: OutputTx) -> Result<()> {
        let closed = Arc::new(AtomicBool::new(false));
        let inner = self.inner.clone();
        let events = self.events.clone();
        let output = Arc::new(Mutex::new(output));
        tokio::spawn(run_session(inner, events, req, output, closed));
        Ok(())
    }

    /// Remplace le canal de sortie (ré-attachement d'un onglet détaché).
    pub async fn set_output(&self, session: &str, output: OutputTx) -> Result<()> {
        let inner = self.inner.lock().await;
        let entry = inner
            .sessions
            .get(session)
            .ok_or_else(|| SshError::NotFound(session.to_string()))?;
        *entry.output.lock().await = output;
        Ok(())
    }

    pub async fn write(&self, session: &str, data: Vec<u8>) -> Result<()> {
        let (channel, closed) = {
            let inner = self.inner.lock().await;
            let entry = inner
                .sessions
                .get(session)
                .ok_or_else(|| SshError::NotFound(session.to_string()))?;
            (entry.channel.clone(), entry.closed.clone())
        };
        if closed.load(Ordering::SeqCst) {
            return Err(SshError::Closed);
        }
        channel.data_bytes(data).await.map_err(|_| SshError::Closed)
    }

    pub async fn resize(&self, session: &str, cols: u32, rows: u32) -> Result<()> {
        let channel = {
            let mut inner = self.inner.lock().await;
            let entry = inner
                .sessions
                .get_mut(session)
                .ok_or_else(|| SshError::NotFound(session.to_string()))?;
            // Mémorise la taille courante : une reconnexion recrée le PTY avec
            // les dernières dimensions, pas celles de l'ouverture initiale.
            entry.request.cols = cols;
            entry.request.rows = rows;
            entry.channel.clone()
        };
        tracing::debug!(session, cols, rows, "resize PTY → window_change");
        channel
            .window_change(cols, rows, 0, 0)
            .await
            .map_err(|_| SshError::Closed)
    }

    /// Session SFTP de la session SSH (ouverte à la première demande, puis
    /// réutilisée). Même connexion et même authentification que le terminal :
    /// second canal `session` + sous-système `sftp`. Le verrou global n'est
    /// tenu que le temps de cloner les poignées.
    pub async fn sftp(&self, session: &str) -> Result<Arc<SftpSession>> {
        let (handle, slot) = {
            let inner = self.inner.lock().await;
            let entry = inner
                .sessions
                .get(session)
                .ok_or_else(|| SshError::NotFound(session.to_string()))?;
            if entry.closed.load(Ordering::SeqCst) {
                return Err(SshError::Closed);
            }
            (entry.handle.clone(), entry.sftp.clone())
        };
        let mut guard = slot.lock().await;
        if let Some(existing) = guard.as_ref() {
            return Ok(existing.clone());
        }
        let channel = handle.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        // `into_stream` consomme le canal : ses messages sont lus en continu
        // par SftpSession (pas de file interne à drainer), et le Handler
        // ignore ce canal (cf. `SshHandler::data`).
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| SshError::Sftp(e.to_string()))?;
        let sftp = Arc::new(sftp);
        *guard = Some(sftp.clone());
        tracing::info!(session, "session SFTP ouverte (même connexion SSH)");
        Ok(sftp)
    }

    /// Poignée SSH + registre des redirections distantes, pour les tunnels (M7).
    pub(crate) async fn tunnel_ctx(
        &self,
        session: &str,
    ) -> Result<(Arc<Handle<SshHandler>>, crate::tunnel::ForwardRegistry)> {
        let inner = self.inner.lock().await;
        let entry = inner
            .sessions
            .get(session)
            .ok_or_else(|| SshError::NotFound(session.to_string()))?;
        if entry.closed.load(Ordering::SeqCst) {
            return Err(SshError::Closed);
        }
        Ok((entry.handle.clone(), inner.forwards.clone()))
    }

    /// La poignée donnée est-elle toujours celle de la session (ni fermée ni
    /// remplacée par une reconnexion) ?
    pub(crate) async fn is_current(&self, session: &str, handle: &Arc<Handle<SshHandler>>) -> bool {
        let inner = self.inner.lock().await;
        inner
            .sessions
            .get(session)
            .is_some_and(|e| Arc::ptr_eq(&e.handle, handle) && !e.closed.load(Ordering::SeqCst))
    }

    /// Oublie la session SFTP (canal cassé) : la prochaine demande en rouvre une.
    pub async fn sftp_invalidate(&self, session: &str) {
        let slot = {
            let inner = self.inner.lock().await;
            inner.sessions.get(session).map(|e| e.sftp.clone())
        };
        if let Some(slot) = slot {
            *slot.lock().await = None;
        }
    }

    pub async fn close(&self, session: &str) -> Result<()> {
        // Le verrou global ne doit JAMAIS être tenu pendant `disconnect` : l'envoi peut attendre la
        // boucle russh, elle-même parfois en attente de ce verrou (handler), et toute nouvelle
        // connexion resterait bloquée sur `inner.lock()` (terminal vide à la reconnexion).
        let entry = {
            let mut inner = self.inner.lock().await;
            let Some(entry) = inner.sessions.remove(session) else {
                return Err(SshError::NotFound(session.to_string()));
            };
            entry.closed.store(true, Ordering::SeqCst);
            inner.states.insert(session.to_string(), "closed".into());
            entry
        };
        disconnect_bounded(&entry.handle, "fermeture").await;
        self.emit(SessionEvent::Closed {
            session: session.to_string(),
            reason: "fermée par l'utilisateur".into(),
        });
        Ok(())
    }

    /// Ferme toutes les sessions (verrouillage du coffre).
    pub async fn close_all(&self) {
        let entries: Vec<(String, SessionEntry)> = {
            let mut inner = self.inner.lock().await;
            let ids: Vec<String> = inner.sessions.keys().cloned().collect();
            let entries = ids
                .into_iter()
                .filter_map(|id| inner.sessions.remove(&id).map(|e| (id, e)))
                .collect();
            inner.states.clear();
            inner.pending.auth.clear();
            inner.pending.hostkey.clear();
            entries
        };
        for (id, entry) in entries {
            entry.closed.store(true, Ordering::SeqCst);
            disconnect_bounded(&entry.handle, "verrouillage").await;
            self.emit(SessionEvent::Closed {
                session: id,
                reason: "coffre verrouillé".into(),
            });
        }
    }

    pub async fn reconnect(&self, session: &str) -> Result<()> {
        let inner_guard = self.inner.lock().await;
        let entry = inner_guard
            .sessions
            .get(session)
            .ok_or_else(|| SshError::NotFound(session.to_string()))?;
        let req = entry.request.clone();
        let output = entry.output.clone();
        entry.closed.store(true, Ordering::SeqCst);
        let closed = Arc::new(AtomicBool::new(false));
        let inner = self.inner.clone();
        let events = self.events.clone();
        drop(inner_guard);
        tokio::spawn(run_session(inner, events, req, output, closed));
        Ok(())
    }

    pub async fn respond_auth(&self, prompt_id: &str, answers: Vec<String>) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let tx = inner
            .pending
            .auth
            .remove(prompt_id)
            .ok_or_else(|| SshError::NotFound(prompt_id.to_string()))?;
        let _ = tx.send(answers);
        Ok(())
    }

    pub async fn respond_hostkey(&self, prompt_id: &str, decision: HostKeyDecision) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let tx = inner
            .pending
            .hostkey
            .remove(prompt_id)
            .ok_or_else(|| SshError::NotFound(prompt_id.to_string()))?;
        let _ = tx.send(decision);
        Ok(())
    }

    /// Identification SSH du serveur de la session (connue dès la première sortie du shell).
    pub async fn server_ident(&self, session: &str) -> Option<String> {
        self.inner.lock().await.idents.get(session).cloned()
    }

    pub async fn states(&self) -> HashMap<String, String> {
        self.inner.lock().await.states.clone()
    }
}

/// Déconnexion avec délai maximum : une boucle russh qui ne répond plus ne doit pas bloquer l'appelant.
async fn disconnect_bounded(handle: &Handle<SshHandler>, reason: &str) {
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        handle.disconnect(Disconnect::ByApplication, reason, ""),
    )
    .await;
}

async fn run_session(
    inner: Arc<Mutex<ManagerInner>>,
    events: EventTx,
    req: OpenRequest,
    output: SharedOutput,
    closed: Arc<AtomicBool>,
) {
    let mut attempt: u32 = 0;
    loop {
        if closed.load(Ordering::SeqCst) {
            return;
        }
        let state = if attempt == 0 {
            "connecting"
        } else {
            "reconnecting"
        };
        let _ = events.send(SessionEvent::State {
            session: req.session_id.clone(),
            state: state.into(),
            detail: None,
        });
        inner
            .lock()
            .await
            .states
            .insert(req.session_id.clone(), state.into());

        match connect_and_open(&inner, &events, &req, &output, &closed).await {
            Ok((handle, hops, channel)) => {
                // russh livre chaque message du canal à la fois au Handler ET à la
                // file interne du Channel (bornée à `channel_buffer_size` = 100,
                // `chan.send().await` dans la boucle de session). Si personne ne lit
                // cette file, la boucle se bloque définitivement au ~100e message :
                // plus aucune sortie, plus de window_change (gel de htop). On
                // sépare donc le canal et on draine la moitié lecture ; la sortie
                // terminal continue de passer par `Handler::data`.
                let (mut read_half, write_half) = channel.split();
                let drain_sid = req.session_id.clone();
                tokio::spawn(async move {
                    while read_half.wait().await.is_some() {}
                    tracing::debug!(session = %drain_sid, "file de lecture du canal fermée");
                });
                let entry = SessionEntry {
                    handle: Arc::new(handle),
                    sftp: Arc::new(Mutex::new(None)),
                    hops,
                    channel: Arc::new(write_half),
                    request: req.clone(),
                    output: output.clone(),
                    closed: closed.clone(),
                };
                {
                    let mut guard = inner.lock().await;
                    guard.sessions.insert(req.session_id.clone(), entry);
                    guard
                        .states
                        .insert(req.session_id.clone(), "connected".into());
                }
                let _ = events.send(SessionEvent::State {
                    session: req.session_id.clone(),
                    state: "connected".into(),
                    detail: None,
                });
                tracing::info!(session = %req.session_id, "session SSH établie");
                return;
            }
            Err(err) => {
                let detail = err.to_string();
                tracing::warn!(
                    session = %req.session_id,
                    attempt,
                    error = %detail,
                    "échec de connexion SSH"
                );
                let _ = events.send(SessionEvent::State {
                    session: req.session_id.clone(),
                    state: "error".into(),
                    detail: Some(detail.clone()),
                });
                let retry = req.reconnect.enabled
                    && !closed.load(Ordering::SeqCst)
                    && attempt < req.reconnect.max_attempts
                    && !matches!(
                        err,
                        SshError::HostKeyRejected
                            | SshError::AuthFailed
                            | SshError::AuthRejected(_)
                    );
                if retry {
                    attempt += 1;
                    let backoff = (1u64 << attempt.min(5)).min(req.reconnect.backoff_max_s as u64);
                    tracing::info!(
                        session = %req.session_id,
                        attempt,
                        backoff_s = backoff,
                        "reconnexion SSH programmée"
                    );
                    tokio::time::sleep(Duration::from_secs(backoff)).await;
                    continue;
                }
                inner
                    .lock()
                    .await
                    .states
                    .insert(req.session_id.clone(), "closed".into());
                let _ = events.send(SessionEvent::Closed {
                    session: req.session_id.clone(),
                    reason: detail,
                });
                return;
            }
        }
    }
}

async fn connect_and_open(
    inner: &Arc<Mutex<ManagerInner>>,
    events: &EventTx,
    req: &OpenRequest,
    output: &SharedOutput,
    closed: &Arc<AtomicBool>,
) -> Result<(
    Handle<SshHandler>,
    Vec<Handle<SshHandler>>,
    Channel<client::Msg>,
)> {
    let chain: Vec<&HopConfig> = req
        .hops
        .iter()
        .chain(std::iter::once(&req.target))
        .collect();
    let mut hop_handles = Vec::new();
    let mut stream: Option<BoxStream> = None;
    let rejected = Arc::new(AtomicBool::new(false));
    let key_changed = Arc::new(AtomicBool::new(false));
    let block_reason: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));

    let shell_channel: Arc<std::sync::Mutex<Option<ChannelId>>> =
        Arc::new(std::sync::Mutex::new(None));
    for (i, hop) in chain.iter().enumerate() {
        let is_last = i == chain.len() - 1;
        let handler = SshHandler {
            session_id: req.session_id.clone(),
            host: hop.host.clone(),
            port: hop.port,
            is_hop: !is_last,
            known_hosts: req.known_hosts.clone(),
            inner: inner.clone(),
            events: events.clone(),
            output: output.clone(),
            closed: closed.clone(),
            request: req.clone(),
            rejected: rejected.clone(),
            key_changed: key_changed.clone(),
            block_reason: block_reason.clone(),
            shell_channel: shell_channel.clone(),
        };
        let config = Arc::new(client::Config {
            keepalive_interval: if req.keepalive_s > 0 {
                Some(Duration::from_secs(u64::from(req.keepalive_s)))
            } else {
                None
            },
            keepalive_max: 3,
            nodelay: true,
            ..Default::default()
        });
        let _ = events.send(SessionEvent::State {
            session: req.session_id.clone(),
            state: "authenticating".into(),
            detail: Some(format!("{}@{}:{}", hop.user, hop.host, hop.port)),
        });
        let connected = match stream.take() {
            Some(s) => client::connect_stream(config, s, handler).await,
            None => client::connect(config, (hop.host.as_str(), hop.port), handler).await,
        };
        let mut handle = match connected {
            Ok(h) => h,
            Err(e) => {
                if key_changed.load(Ordering::SeqCst) {
                    return Err(SshError::HostKeyChanged);
                }
                if let Some(reason) = block_reason.lock().unwrap().take() {
                    return Err(SshError::Connect(reason));
                }
                if rejected.load(Ordering::SeqCst) {
                    return Err(SshError::HostKeyRejected);
                }
                return Err(SshError::Connect(e.to_string()));
            }
        };
        authenticate(&mut handle, hop, inner, events, &req.session_id).await?;
        if is_last {
            let channel = handle.channel_open_session().await?;
            *shell_channel.lock().unwrap() = Some(channel.id());
            channel
                .request_pty(true, "xterm-256color", req.cols, req.rows, 0, 0, &[])
                .await?;
            channel.request_shell(true).await?;
            return Ok((handle, hop_handles, channel));
        }
        let next = chain[i + 1];
        let channel = handle
            .channel_open_direct_tcpip(next.host.clone(), u32::from(next.port), "127.0.0.1", 0)
            .await?;
        stream = Some(Box::new(channel.into_stream()));
        hop_handles.push(handle);
    }
    Err(SshError::Connect("chaîne vide".into()))
}

/// Mémorise les méthodes que le serveur propose encore après un refus.
fn note_offered(offered: &mut Option<Vec<String>>, result: &AuthResult) {
    if let AuthResult::Failure {
        remaining_methods, ..
    } = result
    {
        *offered = Some(
            remaining_methods
                .iter()
                .map(String::from)
                .collect::<Vec<_>>(),
        );
    }
}

/// Dialogue keyboard-interactive ; `Ok(true)` = authentifié.
async fn keyboard_interactive(
    handle: &mut Handle<SshHandler>,
    hop: &HopConfig,
    prefill: Option<&SecretString>,
    inner: &Arc<Mutex<ManagerInner>>,
    events: &EventTx,
    session_id: &str,
) -> Result<bool> {
    let mut response = match handle
        .authenticate_keyboard_interactive_start(hop.user.clone(), None)
        .await
    {
        Ok(r) => r,
        Err(_) => return Ok(false), // méthode non supportée par le serveur
    };
    let mut first = true;
    loop {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let auto = match prefill {
                    Some(p)
                        if first
                            && prompts.len() == 1
                            && prompts[0].prompt.to_lowercase().contains("password") =>
                    {
                        Some(p.expose_secret().to_string())
                    }
                    _ => None,
                };
                let answers = match auto {
                    Some(a) => zeroize::Zeroizing::new(vec![a]),
                    None => {
                        ask_auth_prompt(inner, events, session_id, &name, &instructions, &prompts)
                            .await?
                    }
                };
                first = false;
                response = match handle
                    .authenticate_keyboard_interactive_respond(answers.to_vec())
                    .await
                {
                    Ok(r) => r,
                    Err(_) => return Ok(false),
                };
            }
        }
    }
}

async fn authenticate(
    handle: &mut Handle<SshHandler>,
    hop: &HopConfig,
    inner: &Arc<Mutex<ManagerInner>>,
    events: &EventTx,
    session_id: &str,
) -> Result<()> {
    // Méthodes essayées et méthodes encore proposées par le serveur : sert au message d'erreur
    // et au repli sur une saisie de mot de passe.
    let mut tried: Vec<&'static str> = Vec::new();
    let mut offered: Option<Vec<String>> = None;
    let mut tried_kbd = false;
    for material in &hop.materials {
        match material {
            AuthMaterial::Agent => {
                let Ok(mut agent) = AgentClient::connect_env().await else {
                    tracing::debug!("agent SSH indisponible (SSH_AUTH_SOCK absent ou illisible)");
                    continue;
                };
                let Ok(identities) = agent.request_identities().await else {
                    tracing::debug!("agent SSH : liste des identités refusée");
                    continue;
                };
                if identities.is_empty() {
                    tracing::debug!("agent SSH : aucune identité chargée");
                }
                for identity in &identities {
                    let key = identity.public_key().into_owned();
                    if !tried.contains(&"agent SSH") {
                        tried.push("agent SSH");
                    }
                    match handle
                        .authenticate_publickey_with(hop.user.clone(), key, None, &mut agent)
                        .await
                    {
                        Ok(AuthResult::Success) => return Ok(()),
                        Ok(r) => note_offered(&mut offered, &r),
                        Err(_) => continue,
                    }
                }
            }
            AuthMaterial::Key { pem, passphrase } => {
                let key = decode_secret_key(
                    pem.expose_secret(),
                    passphrase.as_ref().map(|p| p.expose_secret()),
                )
                .map_err(|_| SshError::InvalidKey)?;
                let mut with_alg = PrivateKeyWithHashAlg::new(Arc::new(key.clone()), None);
                if matches!(key.algorithm(), Algorithm::Rsa { .. })
                    && let Ok(Some(Some(alg))) = handle.best_supported_rsa_hash().await
                {
                    with_alg = PrivateKeyWithHashAlg::new(Arc::new(key), Some(alg));
                }
                tried.push("clé privée");
                match handle
                    .authenticate_publickey(hop.user.clone(), with_alg)
                    .await
                {
                    Ok(AuthResult::Success) => return Ok(()),
                    Ok(r) => note_offered(&mut offered, &r),
                    Err(_) => continue,
                }
            }
            AuthMaterial::Password(password) => {
                tried.push("mot de passe");
                match handle
                    .authenticate_password(hop.user.clone(), password.expose_secret())
                    .await
                {
                    Ok(AuthResult::Success) => return Ok(()),
                    Ok(r) => note_offered(&mut offered, &r),
                    Err(_) => continue,
                }
            }
            AuthMaterial::KeyboardInteractive { prefill } => {
                tried.push("keyboard-interactive");
                tried_kbd = true;
                if keyboard_interactive(handle, hop, prefill.as_ref(), inner, events, session_id)
                    .await?
                {
                    return Ok(());
                }
            }
        }
    }

    // Repli : les méthodes configurées ont échoué. Si le serveur accepte encore le mot de passe
    // (ou si on ne sait pas), on le demande à l'utilisateur plutôt que d'échouer sèchement.
    let server_has = |m: &str| offered.as_ref().is_none_or(|o| o.iter().any(|x| x == m));
    if server_has("password") {
        let prompt = russh::client::Prompt {
            prompt: format!("Mot de passe de {}@{} :", hop.user, hop.host),
            echo: false,
        };
        let answers = ask_auth_prompt(
            inner,
            events,
            session_id,
            "Authentification",
            "La méthode configurée a été refusée par le serveur. Saisir le mot de passe pour continuer.",
            &[prompt],
        )
        .await?;
        if let Some(pw) = answers.first().filter(|p| !p.is_empty()) {
            tried.push("mot de passe saisi");
            match handle.authenticate_password(hop.user.clone(), pw).await {
                Ok(AuthResult::Success) => return Ok(()),
                Ok(r) => note_offered(&mut offered, &r),
                Err(_) => {}
            }
        }
    }
    if !tried_kbd
        && offered
            .as_ref()
            .is_some_and(|o| o.iter().any(|x| x == "keyboard-interactive"))
    {
        tried.push("keyboard-interactive");
        if keyboard_interactive(handle, hop, None, inner, events, session_id).await? {
            return Ok(());
        }
    }

    let tried_txt = if tried.is_empty() {
        "aucune (aucun identifiant configuré)".to_string()
    } else {
        tried.join(", ")
    };
    let offered_txt = match &offered {
        Some(o) if !o.is_empty() => o.join(", "),
        _ => "inconnues".to_string(),
    };
    Err(SshError::AuthRejected(format!(
        "{}@{} — méthodes essayées : {tried_txt} ; méthodes proposées par le serveur : {offered_txt}",
        hop.user, hop.host
    )))
}

async fn ask_auth_prompt(
    inner: &Arc<Mutex<ManagerInner>>,
    events: &EventTx,
    session_id: &str,
    name: &str,
    instructions: &str,
    prompts: &[russh::client::Prompt],
) -> Result<zeroize::Zeroizing<Vec<String>>> {
    let prompt_id = Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    inner
        .lock()
        .await
        .pending
        .auth
        .insert(prompt_id.clone(), tx);
    let _ = events.send(SessionEvent::AuthPrompt {
        session: session_id.to_string(),
        prompt_id,
        name: name.to_string(),
        instructions: instructions.to_string(),
        prompts: prompts
            .iter()
            .map(|p| PromptDto {
                text: p.prompt.clone(),
                echo: p.echo,
            })
            .collect(),
    });
    match tokio::time::timeout(Duration::from_secs(300), rx).await {
        Ok(Ok(answers)) => Ok(zeroize::Zeroizing::new(answers)),
        _ => Err(SshError::PromptTimeout),
    }
}

pub(crate) struct SshHandler {
    session_id: String,
    host: String,
    port: u16,
    is_hop: bool,
    known_hosts: PathBuf,
    inner: Arc<Mutex<ManagerInner>>,
    events: EventTx,
    output: SharedOutput,
    closed: Arc<AtomicBool>,
    request: OpenRequest,
    rejected: Arc<AtomicBool>,
    /// Clé d'hôte changée : blocage dur, aucune acceptation possible depuis la session.
    key_changed: Arc<AtomicBool>,
    /// Motif d'un refus décidé par la vérification de clé d'hôte (known_hosts illisible, clé révoquée).
    block_reason: Arc<std::sync::Mutex<Option<String>>>,
    /// Identifiant du canal shell : seules ses données vont au terminal. Les
    /// autres canaux (SFTP) ont leurs propres lecteurs ; sans ce filtre, les
    /// octets SFTP seraient injectés dans l'affichage du terminal.
    shell_channel: Arc<std::sync::Mutex<Option<ChannelId>>>,
}

impl SshHandler {
    fn is_shell(&self, channel: ChannelId) -> bool {
        *self.shell_channel.lock().unwrap() == Some(channel)
    }

    async fn verify_host_key(&mut self, key: &PublicKeyOrCertificate) -> bool {
        let public = key.public_key();
        let key_type = public.algorithm().as_str().to_string();
        let Ok(blob) = public.to_bytes() else {
            return false;
        };
        let key_b64 = B64.encode(&blob);
        let fingerprint = format!("{}", public.fingerprint(HashAlg::Sha256));
        let status = knownhosts::check(
            &self.known_hosts,
            &self.host,
            self.port,
            &key_type,
            &key_b64,
        );
        // Fail-closed : un known_hosts illisible ne doit JAMAIS retomber sur « première connexion ».
        let status = match status {
            Ok(s) => s,
            Err(e) => {
                *self.block_reason.lock().unwrap() = Some(format!(
                    "known_hosts illisible ({e}) : connexion refusée par sécurité"
                ));
                return false;
            }
        };
        let (kind, old_fingerprint) = match status {
            KnownHostStatus::Match => return true,
            KnownHostStatus::Revoked => {
                *self.block_reason.lock().unwrap() = Some(
                    "clé d'hôte révoquée (@revoked dans known_hosts) : connexion refusée".into(),
                );
                return false;
            }
            KnownHostStatus::Unknown => ("unknown", None),
            KnownHostStatus::Changed { old_fingerprint } => {
                // Blocage dur (anti-MITM) : aucun prompt d'acceptation n'est enregistré, donc
                // aucune réponse ne peut débloquer la connexion. Le remplacement passe par
                // l'oubli explicite de l'hôte (Paramètres → known_hosts).
                self.key_changed.store(true, Ordering::SeqCst);
                let _ = self.events.send(SessionEvent::HostKeyPrompt {
                    session: self.session_id.clone(),
                    prompt_id: Uuid::new_v4().to_string(),
                    host: self.host.clone(),
                    port: self.port,
                    kind: "changed".to_string(),
                    fingerprint,
                    old_fingerprint: Some(old_fingerprint),
                });
                return false;
            }
        };
        let prompt_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.inner
            .lock()
            .await
            .pending
            .hostkey
            .insert(prompt_id.clone(), tx);
        let _ = self.events.send(SessionEvent::HostKeyPrompt {
            session: self.session_id.clone(),
            prompt_id,
            host: self.host.clone(),
            port: self.port,
            kind: kind.to_string(),
            fingerprint,
            old_fingerprint,
        });
        let decision = match tokio::time::timeout(Duration::from_secs(300), rx).await {
            Ok(Ok(d)) => d,
            _ => {
                self.rejected.store(true, Ordering::SeqCst);
                return false;
            }
        };
        if decision.accept && decision.remember {
            let _ = knownhosts::learn(
                &self.known_hosts,
                &self.host,
                self.port,
                &key_type,
                &key_b64,
            );
        }
        if !decision.accept {
            self.rejected.store(true, Ordering::SeqCst);
        }
        decision.accept
    }
}

impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        Ok(self.verify_host_key(server_public_key).await)
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        if !self.is_hop && self.is_shell(channel) {
            let mut inner = self.inner.lock().await;
            if !inner.idents.contains_key(&self.session_id) {
                let id = String::from_utf8_lossy(session.remote_sshid()).into_owned();
                inner.idents.insert(self.session_id.clone(), id);
            }
            drop(inner);
            let _ = self.output.lock().await.send(data.to_vec());
        }
        Ok(())
    }

    async fn extended_data(
        &mut self,
        channel: ChannelId,
        _ext: u32,
        data: &[u8],
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        if !self.is_hop && self.is_shell(channel) {
            let _ = self.output.lock().await.send(data.to_vec());
        }
        Ok(())
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<client::Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        // Tunnel distant (-R) : route la connexion vers le tunnel du port annoncé ;
        // sans tunnel correspondant (ou sur un saut intermédiaire) : refus.
        let target = if self.is_hop {
            None
        } else {
            let registry = self.inner.lock().await.forwards.clone();
            let reg = registry.lock().unwrap();
            reg.get(&(self.session_id.clone(), connected_port)).cloned()
        };
        match target {
            Some(tx) => {
                reply.accept().await;
                let _ = tx.send(crate::tunnel::ForwardedConn { channel });
            }
            None => {
                reply
                    .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                    .await
            }
        }
        Ok(())
    }

    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        let _ = self.events.send(SessionEvent::Banner {
            session: self.session_id.clone(),
            text: banner.to_string(),
        });
        Ok(())
    }

    async fn exit_status(
        &mut self,
        _channel: ChannelId,
        exit_status: u32,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        let _ = self.events.send(SessionEvent::ExitStatus {
            session: self.session_id.clone(),
            code: exit_status,
        });
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        _channel: ChannelId,
        _session: &mut client::Session,
    ) -> std::result::Result<(), Self::Error> {
        Ok(())
    }

    async fn disconnected(
        &mut self,
        reason: russh::client::DisconnectReason<Self::Error>,
    ) -> std::result::Result<(), Self::Error> {
        if self.is_hop || self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        let detail = match &reason {
            russh::client::DisconnectReason::ReceivedDisconnect(info) => {
                format!("déconnexion serveur : {info:?}")
            }
            russh::client::DisconnectReason::Error(e) => format!("erreur : {e}"),
        };
        self.closed.store(true, Ordering::SeqCst);
        self.inner.lock().await.sessions.remove(&self.session_id);
        let _ = self.events.send(SessionEvent::Closed {
            session: self.session_id.clone(),
            reason: detail,
        });
        if self.request.reconnect.enabled {
            let closed = Arc::new(AtomicBool::new(false));
            tokio::spawn(run_session(
                self.inner.clone(),
                self.events.clone(),
                self.request.clone(),
                self.output.clone(),
                closed,
            ));
        }
        Ok(())
    }
}

/// Identité exposée par l'agent SSH local (clé publique seulement ; jamais de secret).
#[derive(Clone, Debug)]
pub struct AgentKey {
    pub algorithm: String,
    pub fingerprint: String,
    pub comment: String,
    pub certificate: bool,
}

/// État de l'agent SSH local (`SSH_AUTH_SOCK`) : `None` si indisponible, sinon ses identités
/// (y compris les clés matérielles `sk-…` FIDO2, dont la signature reste dans l'agent/le jeton).
pub async fn agent_keys() -> Option<Vec<AgentKey>> {
    use russh::keys::agent::AgentIdentity;
    let mut agent = AgentClient::connect_env().await.ok()?;
    let ids = agent.request_identities().await.ok()?;
    Some(
        ids.iter()
            .map(|id| {
                let key = id.public_key();
                let (comment, certificate) = match id {
                    AgentIdentity::PublicKey { comment, .. } => (comment.clone(), false),
                    AgentIdentity::Certificate { comment, .. } => (comment.clone(), true),
                };
                AgentKey {
                    algorithm: key.algorithm().as_str().to_string(),
                    fingerprint: format!("{}", key.fingerprint(HashAlg::Sha256)),
                    comment,
                    certificate,
                }
            })
            .collect(),
    )
}
