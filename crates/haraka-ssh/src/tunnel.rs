//! Tunnels SSH (jalon M7) : redirections locale (-L), distante (-R) et
//! dynamique (-D, SOCKS5) au travers d'une session SSH existante.
//!
//! Garanties :
//! - écoute par défaut sur la boucle locale (`127.0.0.1`) ; une adresse non
//!   loopback n'est utilisée que si la spec la demande explicitement ;
//! - concurrence bornée par tunnel (256) et délais de connexion ;
//! - arrêt propre : l'arrêt d'un tunnel ferme son port et toutes ses
//!   connexions ; la fin de la session (fermeture, perte, reconnexion) arrête
//!   ses tunnels dans la seconde ;
//! - aucun contenu transféré n'est journalisé.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use russh::client::{Handle, Msg};
use russh::{Channel, ChannelOpenFailure};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};

use haraka_core::model::TunnelSpec;

use crate::error::{Result, SshError};
use crate::session::{SessionManager, SshHandler};

/// Connexions simultanées maximales par tunnel.
pub const MAX_CONNECTIONS: usize = 256;
const OPEN_TIMEOUT: Duration = Duration::from_secs(15);
const LOCAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SOCKS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelKind {
    Local,
    Remote,
    Dynamic,
}

impl FromStr for TunnelKind {
    type Err = SshError;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "local" => Ok(Self::Local),
            "remote" => Ok(Self::Remote),
            "dynamic" => Ok(Self::Dynamic),
            other => Err(SshError::Tunnel(format!(
                "type de tunnel inconnu « {other} »"
            ))),
        }
    }
}

/// Définition d'un tunnel (reflète `TunnelSpec`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelDef {
    pub id: String,
    pub kind: TunnelKind,
    pub bind_addr: String,
    pub bind_port: u16,
    pub dest_host: String,
    pub dest_port: u16,
}

impl TunnelDef {
    pub fn from_spec(spec: &TunnelSpec) -> Result<Self> {
        Ok(Self {
            id: spec.id.clone(),
            kind: spec.kind.parse()?,
            bind_addr: spec.bind_addr.clone(),
            bind_port: spec.bind_port,
            dest_host: spec.dest_host.clone().unwrap_or_default(),
            dest_port: spec.dest_port.unwrap_or(0),
        })
    }

    fn validate(&self) -> Result<()> {
        if self.id.is_empty() {
            return Err(SshError::Tunnel("identifiant de tunnel vide".into()));
        }
        if self.kind != TunnelKind::Dynamic
            && (self.dest_host.trim().is_empty() || self.dest_port == 0)
        {
            return Err(SshError::Tunnel(
                "hôte et port de destination requis".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelState {
    Starting,
    Listening,
    Error,
}

/// Instantané d'un tunnel (liste et événements).
#[derive(Debug, Clone, Serialize)]
pub struct TunnelInfo {
    pub id: String,
    pub session: String,
    pub kind: TunnelKind,
    pub bind_addr: String,
    /// Port réellement écouté (résolu si 0 demandé).
    pub bind_port: u16,
    pub dest_host: String,
    pub dest_port: u16,
    pub state: TunnelState,
    pub error: Option<String>,
    /// Connexions actives.
    pub active: u32,
    /// Octets de l'initiateur vers la destination.
    pub bytes_out: u64,
    /// Octets de la destination vers l'initiateur.
    pub bytes_in: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TunnelEvent {
    Changed { info: TunnelInfo },
    Removed { session: String, id: String },
}

/// Connexion `forwarded-tcpip` reçue du serveur (tunnel distant).
pub(crate) struct ForwardedConn {
    pub channel: Channel<Msg>,
}

/// Registre (session, port distant) -> tunnel distant, partagé avec `SshHandler`.
pub(crate) type ForwardRegistry =
    Arc<Mutex<HashMap<(String, u32), mpsc::UnboundedSender<ForwardedConn>>>>;

struct Shared {
    def: TunnelDef,
    session: String,
    bind_addr: String,
    status: Mutex<Status>,
    active: AtomicU32,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
}

struct Status {
    state: TunnelState,
    error: Option<String>,
    port: u16,
}

impl Shared {
    fn info(&self) -> TunnelInfo {
        let st = self.status.lock().unwrap();
        TunnelInfo {
            id: self.def.id.clone(),
            session: self.session.clone(),
            kind: self.def.kind,
            bind_addr: self.bind_addr.clone(),
            bind_port: st.port,
            dest_host: self.def.dest_host.clone(),
            dest_port: self.def.dest_port,
            state: st.state,
            error: st.error.clone(),
            active: self.active.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
        }
    }

    fn set_state(&self, state: TunnelState, error: Option<String>, port: Option<u16>) {
        let mut st = self.status.lock().unwrap();
        st.state = state;
        st.error = error;
        if let Some(p) = port {
            st.port = p;
        }
    }

    fn counters(&self) -> (u32, u64, u64) {
        (
            self.active.load(Ordering::Relaxed),
            self.bytes_out.load(Ordering::Relaxed),
            self.bytes_in.load(Ordering::Relaxed),
        )
    }
}

/// Décrémente le compteur de connexions actives à la fin de la connexion.
struct ActiveGuard(Arc<Shared>);
impl ActiveGuard {
    fn new(shared: &Arc<Shared>) -> Self {
        shared.active.fetch_add(1, Ordering::Relaxed);
        Self(shared.clone())
    }
}
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Flux de l'initiateur avec comptage d'octets (lu = sortant, écrit = entrant).
struct Counted<S> {
    inner: S,
    shared: Arc<Shared>,
}

impl<S: AsyncRead + Unpin> AsyncRead for Counted<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &r {
            let n = buf.filled().len() - before;
            self.shared.bytes_out.fetch_add(n as u64, Ordering::Relaxed);
        }
        r
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Counted<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let r = Pin::new(&mut self.inner).poll_write(cx, data);
        if let Poll::Ready(Ok(n)) = &r {
            self.shared.bytes_in.fetch_add(*n as u64, Ordering::Relaxed);
        }
        r
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

type Key = (String, String);

struct Entry {
    shared: Arc<Shared>,
    handle: Arc<Handle<SshHandler>>,
    generation: u64,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

struct Inner {
    sessions: Arc<SessionManager>,
    tunnels: Mutex<HashMap<Key, Entry>>,
    events: mpsc::UnboundedSender<TunnelEvent>,
    next_gen: AtomicU64,
}

impl Inner {
    fn emit(&self, ev: TunnelEvent) {
        let _ = self.events.send(ev);
    }
}

#[derive(Clone)]
pub struct TunnelManager {
    inner: Arc<Inner>,
}

enum Runner {
    Local(TcpListener),
    Dynamic(TcpListener),
    Remote {
        rx: mpsc::UnboundedReceiver<ForwardedConn>,
        bind_addr: String,
        port: u32,
    },
}

enum Incoming {
    Tcp(TcpStream, SocketAddr),
    Forwarded(ForwardedConn),
    Failed(std::io::Error),
    Gone,
}

impl Runner {
    async fn next(&mut self) -> Incoming {
        match self {
            Runner::Local(l) | Runner::Dynamic(l) => match l.accept().await {
                Ok((s, a)) => Incoming::Tcp(s, a),
                Err(e) => Incoming::Failed(e),
            },
            Runner::Remote { rx, .. } => match rx.recv().await {
                Some(c) => Incoming::Forwarded(c),
                None => Incoming::Gone,
            },
        }
    }
}

fn parse_bind(addr: &str) -> Result<IpAddr> {
    let a = addr.trim();
    if a.is_empty() || a.eq_ignore_ascii_case("localhost") {
        return Ok(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    a.trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .map_err(|_| SshError::Tunnel(format!("adresse d'écoute invalide « {a} »")))
}

fn bind_error(addr: &SocketAddr, e: &std::io::Error) -> SshError {
    let why = match e.kind() {
        std::io::ErrorKind::AddrInUse => "port déjà utilisé".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission refusée".to_string(),
        std::io::ErrorKind::AddrNotAvailable => "adresse indisponible".to_string(),
        _ => e.to_string(),
    };
    SshError::Tunnel(format!("impossible d'écouter sur {addr} : {why}"))
}

impl TunnelManager {
    pub fn new(sessions: Arc<SessionManager>) -> (Self, mpsc::UnboundedReceiver<TunnelEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                inner: Arc::new(Inner {
                    sessions,
                    tunnels: Mutex::new(HashMap::new()),
                    events: tx,
                    next_gen: AtomicU64::new(1),
                }),
            },
            rx,
        )
    }

    /// Tunnels vivants (tous états), triés par session puis identifiant.
    pub fn list(&self) -> Vec<TunnelInfo> {
        let mut v: Vec<TunnelInfo> = self
            .inner
            .tunnels
            .lock()
            .unwrap()
            .values()
            .map(|e| e.shared.info())
            .collect();
        v.sort_by(|a, b| (&a.session, &a.id).cmp(&(&b.session, &b.id)));
        v
    }

    /// Démarre un tunnel. Une erreur de mise en écoute (port occupé, serveur qui
    /// refuse…) n'est pas une erreur de l'appel : le tunnel est enregistré à
    /// l'état `error` et son instantané est retourné. Démarrer un tunnel déjà actif
    /// (même session et même id) renvoie l'existant ; un tunnel d'une connexion
    /// SSH précédente (reconnexion) est remplacé.
    pub async fn start(&self, session: &str, def: TunnelDef) -> Result<TunnelInfo> {
        def.validate()?;
        let (handle, registry) = self.inner.sessions.tunnel_ctx(session).await?;
        let key: Key = (session.to_string(), def.id.clone());
        let generation = self.inner.next_gen.fetch_add(1, Ordering::Relaxed);
        let bind_addr = match def.kind {
            TunnelKind::Remote if def.bind_addr.trim().is_empty() => "127.0.0.1".to_string(),
            TunnelKind::Remote => def.bind_addr.trim().to_string(),
            _ => parse_bind(&def.bind_addr)
                .map(|a| a.to_string())
                .unwrap_or_else(|_| def.bind_addr.trim().to_string()),
        };
        let shared = Arc::new(Shared {
            bind_addr,
            session: session.to_string(),
            status: Mutex::new(Status {
                state: TunnelState::Starting,
                error: None,
                port: def.bind_port,
            }),
            active: AtomicU32::new(0),
            bytes_in: AtomicU64::new(0),
            bytes_out: AtomicU64::new(0),
            def,
        });
        let (stop_tx, stop_rx) = watch::channel(false);
        let stale = {
            let mut map = self.inner.tunnels.lock().unwrap();
            let mut stale = None;
            if let Some(existing) = map.get(&key) {
                let alive = existing.shared.info().state != TunnelState::Error
                    && Arc::ptr_eq(&existing.handle, &handle)
                    && !handle.is_closed();
                if alive {
                    return Ok(existing.shared.info());
                }
                stale = map.remove(&key);
            }
            map.insert(
                key.clone(),
                Entry {
                    shared: shared.clone(),
                    handle: handle.clone(),
                    generation,
                    stop: stop_tx,
                    task: None,
                },
            );
            stale
        };
        if let Some(old) = stale {
            shutdown_entry(old).await;
        }
        self.inner.emit(TunnelEvent::Changed {
            info: shared.info(),
        });

        match setup(session, &shared, &handle, &registry).await {
            Ok((runner, port)) => {
                let leftover = {
                    let mut map = self.inner.tunnels.lock().unwrap();
                    match map.get_mut(&key) {
                        Some(e) if e.generation == generation => {
                            shared.set_state(TunnelState::Listening, None, Some(port));
                            e.task = Some(tokio::spawn(run(
                                self.inner.clone(),
                                key.clone(),
                                generation,
                                shared.clone(),
                                handle.clone(),
                                registry.clone(),
                                runner,
                                stop_rx,
                            )));
                            None
                        }
                        _ => Some(runner),
                    }
                };
                if let Some(runner) = leftover {
                    // Arrêté pendant le démarrage.
                    teardown(&runner, session, &handle, &registry).await;
                    return Err(SshError::Closed);
                }
            }
            Err(e) => {
                tracing::warn!(session, tunnel = %shared.def.id, "tunnel en erreur : {e}");
                shared.set_state(TunnelState::Error, Some(e.to_string()), None);
            }
        }
        let info = shared.info();
        self.inner.emit(TunnelEvent::Changed { info: info.clone() });
        Ok(info)
    }

    /// Arrête un tunnel (port libéré et connexions fermées au retour).
    pub async fn stop(&self, session: &str, id: &str) -> Result<()> {
        let entry = self
            .inner
            .tunnels
            .lock()
            .unwrap()
            .remove(&(session.to_string(), id.to_string()))
            .ok_or_else(|| SshError::Tunnel("tunnel introuvable".into()))?;
        shutdown_entry(entry).await;
        self.inner.emit(TunnelEvent::Removed {
            session: session.to_string(),
            id: id.to_string(),
        });
        Ok(())
    }

    /// Arrête tous les tunnels de toutes les sessions (verrouillage du coffre).
    pub async fn stop_everything(&self) {
        let keys: Vec<Key> = self.inner.tunnels.lock().unwrap().keys().cloned().collect();
        for (session, id) in keys {
            let _ = self.stop(&session, &id).await;
        }
    }

    /// Arrête tous les tunnels d'une session ; retourne leur nombre.
    pub async fn stop_all(&self, session: &str) -> usize {
        let ids: Vec<String> = self
            .inner
            .tunnels
            .lock()
            .unwrap()
            .keys()
            .filter(|(s, _)| s == session)
            .map(|(_, id)| id.clone())
            .collect();
        let mut n = 0;
        for id in ids {
            if self.stop(session, &id).await.is_ok() {
                n += 1;
            }
        }
        n
    }
}

async fn shutdown_entry(e: Entry) {
    let _ = e.stop.send(true);
    if let Some(t) = e.task {
        let _ = t.await;
    }
}

/// Met le tunnel en écoute (local/dynamique) ou demande la redirection au serveur.
async fn setup(
    session: &str,
    shared: &Arc<Shared>,
    handle: &Arc<Handle<SshHandler>>,
    registry: &ForwardRegistry,
) -> Result<(Runner, u16)> {
    let def = &shared.def;
    match def.kind {
        TunnelKind::Local | TunnelKind::Dynamic => {
            let ip = parse_bind(&def.bind_addr)?;
            let addr = SocketAddr::new(ip, def.bind_port);
            let listener = TcpListener::bind(addr)
                .await
                .map_err(|e| bind_error(&addr, &e))?;
            let port = listener.local_addr()?.port();
            Ok(if def.kind == TunnelKind::Local {
                (Runner::Local(listener), port)
            } else {
                (Runner::Dynamic(listener), port)
            })
        }
        TunnelKind::Remote => {
            let (tx, rx) = mpsc::unbounded_channel();
            let requested = u32::from(def.bind_port);
            let refused = || {
                SshError::Tunnel(
                    "le serveur a refusé la redirection distante (port occupé ou redirection interdite)"
                        .into(),
                )
            };
            let key = (session.to_string(), requested);
            if requested != 0 {
                // Port déjà redirigé par un autre tunnel de la session : ne pas
                // écraser sa route.
                let mut reg = registry.lock().unwrap();
                if reg.contains_key(&key) {
                    return Err(refused());
                }
                reg.insert(key.clone(), tx.clone());
            }
            let reply = tokio::time::timeout(
                OPEN_TIMEOUT,
                handle.tcpip_forward(shared.bind_addr.clone(), requested),
            )
            .await;
            let forget = || {
                if requested != 0 {
                    registry.lock().unwrap().remove(&key);
                }
            };
            let port = match reply {
                Ok(Ok(p)) => {
                    if p == 0 {
                        requested
                    } else {
                        p
                    }
                }
                Ok(Err(russh::Error::RequestDenied)) => {
                    forget();
                    return Err(refused());
                }
                Ok(Err(e)) => {
                    forget();
                    return Err(e.into());
                }
                Err(_) => {
                    forget();
                    return Err(SshError::Tunnel(
                        "délai dépassé : le serveur n'a pas répondu à la redirection distante"
                            .into(),
                    ));
                }
            };
            if port != requested {
                let mut reg = registry.lock().unwrap();
                reg.remove(&(session.to_string(), requested));
                reg.insert((session.to_string(), port), tx);
            }
            Ok((
                Runner::Remote {
                    rx,
                    bind_addr: shared.bind_addr.clone(),
                    port,
                },
                u16::try_from(port).unwrap_or(0),
            ))
        }
    }
}

/// Libère les ressources d'un `Runner` (registre + annulation côté serveur).
async fn teardown(
    runner: &Runner,
    session: &str,
    handle: &Arc<Handle<SshHandler>>,
    registry: &ForwardRegistry,
) {
    if let Runner::Remote {
        bind_addr, port, ..
    } = runner
    {
        registry
            .lock()
            .unwrap()
            .remove(&(session.to_string(), *port));
        if !handle.is_closed() {
            let _ = tokio::time::timeout(
                CANCEL_TIMEOUT,
                handle.cancel_tcpip_forward(bind_addr.clone(), *port),
            )
            .await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    inner: Arc<Inner>,
    key: Key,
    generation: u64,
    shared: Arc<Shared>,
    handle: Arc<Handle<SshHandler>>,
    registry: ForwardRegistry,
    mut runner: Runner,
    mut stop: watch::Receiver<bool>,
) {
    let sem = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut set: JoinSet<()> = JoinSet::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last = shared.counters();
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = tick.tick() => {
                if handle.is_closed() || !inner.sessions.is_current(&key.0, &handle).await {
                    break;
                }
                let c = shared.counters();
                if c != last {
                    last = c;
                    inner.emit(TunnelEvent::Changed { info: shared.info() });
                }
            }
            Some(_) = set.join_next(), if !set.is_empty() => {}
            incoming = runner.next() => match incoming {
                Incoming::Gone => break,
                Incoming::Failed(e) => {
                    tracing::warn!(tunnel = %key.1, "accept impossible : {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Incoming::Tcp(sock, peer) => {
                    let Ok(permit) = sem.clone().try_acquire_owned() else {
                        tracing::warn!(tunnel = %key.1, "limite de connexions atteinte");
                        continue;
                    };
                    let guard = ActiveGuard::new(&shared);
                    let handle = handle.clone();
                    let shared = shared.clone();
                    if matches!(runner, Runner::Dynamic(_)) {
                        set.spawn(socks_conn(handle, sock, peer, shared, guard, permit));
                    } else {
                        set.spawn(local_conn(handle, sock, peer, shared, guard, permit));
                    }
                }
                Incoming::Forwarded(fc) => {
                    let Ok(permit) = sem.clone().try_acquire_owned() else {
                        tracing::warn!(tunnel = %key.1, "limite de connexions atteinte");
                        continue;
                    };
                    let guard = ActiveGuard::new(&shared);
                    set.spawn(remote_conn(fc, shared.clone(), guard, permit));
                }
            }
        }
    }
    teardown(&runner, &key.0, &handle, &registry).await;
    set.shutdown().await;
    drop(runner);
    let removed = {
        let mut map = inner.tunnels.lock().unwrap();
        if map.get(&key).is_some_and(|e| e.generation == generation) {
            map.remove(&key);
            true
        } else {
            false
        }
    };
    if removed {
        inner.emit(TunnelEvent::Removed {
            session: key.0,
            id: key.1,
        });
    }
}

async fn open_direct(
    handle: &Handle<SshHandler>,
    host: &str,
    port: u16,
    peer: SocketAddr,
) -> std::result::Result<Channel<Msg>, russh::Error> {
    match tokio::time::timeout(
        OPEN_TIMEOUT,
        handle.channel_open_direct_tcpip(
            host.to_string(),
            u32::from(port),
            peer.ip().to_string(),
            u32::from(peer.port()),
        ),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => Err(russh::Error::ConnectionTimeout),
    }
}

async fn local_conn(
    handle: Arc<Handle<SshHandler>>,
    sock: TcpStream,
    peer: SocketAddr,
    shared: Arc<Shared>,
    _guard: ActiveGuard,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    let _ = sock.set_nodelay(true);
    let chan = match open_direct(&handle, &shared.def.dest_host, shared.def.dest_port, peer).await {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(tunnel = %shared.def.id, "ouverture direct-tcpip refusée : {e}");
            return;
        }
    };
    let mut a = Counted {
        inner: sock,
        shared: shared.clone(),
    };
    let mut b = chan.into_stream();
    let _ = copy_bidirectional(&mut a, &mut b).await;
}

async fn socks_conn(
    handle: Arc<Handle<SshHandler>>,
    mut sock: TcpStream,
    peer: SocketAddr,
    shared: Arc<Shared>,
    _guard: ActiveGuard,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    let _ = sock.set_nodelay(true);
    let target =
        match tokio::time::timeout(SOCKS_HANDSHAKE_TIMEOUT, socks5_handshake(&mut sock)).await {
            Ok(Ok(t)) => t,
            _ => return,
        };
    let chan = match open_direct(&handle, &target.0, target.1, peer).await {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(tunnel = %shared.def.id, "ouverture direct-tcpip refusée : {e}");
            let _ = socks5_reply(&mut sock, reply_for(&e)).await;
            return;
        }
    };
    if socks5_reply(&mut sock, REP_OK).await.is_err() {
        return;
    }
    let mut a = Counted {
        inner: sock,
        shared: shared.clone(),
    };
    let mut b = chan.into_stream();
    let _ = copy_bidirectional(&mut a, &mut b).await;
}

async fn remote_conn(
    fc: ForwardedConn,
    shared: Arc<Shared>,
    _guard: ActiveGuard,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    let dest = (shared.def.dest_host.clone(), shared.def.dest_port);
    let tcp = match tokio::time::timeout(LOCAL_CONNECT_TIMEOUT, TcpStream::connect(dest)).await {
        Ok(Ok(s)) => s,
        _ => {
            tracing::debug!(tunnel = %shared.def.id, "connexion à la destination locale impossible");
            return; // le canal est fermé au drop
        }
    };
    let _ = tcp.set_nodelay(true);
    let mut a = Counted {
        inner: fc.channel.into_stream(),
        shared: shared.clone(),
    };
    let mut b = tcp;
    let _ = copy_bidirectional(&mut a, &mut b).await;
}

// ---------------------------------------------------------------- SOCKS5

pub(crate) const REP_OK: u8 = 0;
pub(crate) const REP_GENERAL: u8 = 1;
pub(crate) const REP_NOT_ALLOWED: u8 = 2;
pub(crate) const REP_HOST_UNREACHABLE: u8 = 4;
pub(crate) const REP_REFUSED: u8 = 5;
pub(crate) const REP_CMD_UNSUPPORTED: u8 = 7;
pub(crate) const REP_ATYP_UNSUPPORTED: u8 = 8;

/// Code de réponse SOCKS5 correspondant à un échec d'ouverture `direct-tcpip`.
pub(crate) fn reply_for(e: &russh::Error) -> u8 {
    match e {
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed) => REP_REFUSED,
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited) => {
            REP_NOT_ALLOWED
        }
        russh::Error::ConnectionTimeout => REP_HOST_UNREACHABLE,
        _ => REP_GENERAL,
    }
}

pub(crate) async fn socks5_reply<S: AsyncWrite + Unpin>(
    s: &mut S,
    code: u8,
) -> std::io::Result<()> {
    s.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    s.flush().await
}

fn bad(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

/// Négociation SOCKS5 (RFC 1928) côté serveur : sans authentification, CONNECT
/// uniquement. Retourne la cible `(hôte, port)`. Sur refus, la réponse
/// d'erreur adéquate est déjà envoyée et une erreur est retournée.
pub(crate) async fn socks5_handshake<S>(s: &mut S) -> std::io::Result<(String, u16)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut head = [0u8; 2];
    s.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(bad("version SOCKS non prise en charge"));
    }
    let mut methods = vec![0u8; usize::from(head[1])];
    s.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        s.write_all(&[5, 0xFF]).await?;
        return Err(bad("aucune méthode d'authentification acceptable"));
    }
    s.write_all(&[5, 0]).await?;

    let mut req = [0u8; 4];
    s.read_exact(&mut req).await?;
    if req[0] != 5 {
        return Err(bad("version SOCKS invalide"));
    }
    let atyp = req[3];
    let host = match atyp {
        1 => {
            let mut a = [0u8; 4];
            s.read_exact(&mut a).await?;
            Ipv4Addr::from(a).to_string()
        }
        4 => {
            let mut a = [0u8; 16];
            s.read_exact(&mut a).await?;
            Ipv6Addr::from(a).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            let mut name = vec![0u8; usize::from(len[0])];
            s.read_exact(&mut name).await?;
            match String::from_utf8(name) {
                Ok(n) if !n.is_empty() => n,
                _ => {
                    socks5_reply(s, REP_GENERAL).await?;
                    return Err(bad("nom d'hôte invalide"));
                }
            }
        }
        _ => {
            socks5_reply(s, REP_ATYP_UNSUPPORTED).await?;
            return Err(bad("type d'adresse non pris en charge"));
        }
    };
    let mut port = [0u8; 2];
    s.read_exact(&mut port).await?;
    if req[1] != 1 {
        socks5_reply(s, REP_CMD_UNSUPPORTED).await?;
        return Err(bad(
            "commande SOCKS non prise en charge (CONNECT uniquement)",
        ));
    }
    Ok((host, u16::from_be_bytes(port)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    async fn run_handshake(input: Vec<u8>) -> (std::io::Result<(String, u16)>, Vec<u8>) {
        let (mut client, mut server) = duplex(4096);
        client.write_all(&input).await.unwrap();
        client.shutdown().await.unwrap();
        let r = socks5_handshake(&mut server).await;
        drop(server);
        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        (r, out)
    }

    #[tokio::test]
    async fn socks5_ipv4_connect() {
        let mut v = vec![5, 1, 0, 5, 1, 0, 1, 10, 1, 2, 3];
        v.extend_from_slice(&8080u16.to_be_bytes());
        let (r, out) = run_handshake(v).await;
        assert_eq!(r.unwrap(), ("10.1.2.3".to_string(), 8080));
        assert_eq!(out, vec![5, 0]);
    }

    #[tokio::test]
    async fn socks5_domain_connect() {
        let mut v = vec![5, 2, 2, 0, 5, 1, 0, 3, 11];
        v.extend_from_slice(b"example.org");
        v.extend_from_slice(&443u16.to_be_bytes());
        let (r, _) = run_handshake(v).await;
        assert_eq!(r.unwrap(), ("example.org".to_string(), 443));
    }

    #[tokio::test]
    async fn socks5_ipv6_connect() {
        let mut v = vec![5, 1, 0, 5, 1, 0, 4];
        v.extend_from_slice(&Ipv6Addr::LOCALHOST.octets());
        v.extend_from_slice(&22u16.to_be_bytes());
        let (r, _) = run_handshake(v).await;
        assert_eq!(r.unwrap(), ("::1".to_string(), 22));
    }

    #[tokio::test]
    async fn socks5_refuses_bind_and_udp() {
        for cmd in [2u8, 3] {
            let mut v = vec![5, 1, 0, 5, cmd, 0, 1, 1, 2, 3, 4];
            v.extend_from_slice(&1u16.to_be_bytes());
            let (r, out) = run_handshake(v).await;
            assert!(r.is_err());
            assert_eq!(out[3], REP_CMD_UNSUPPORTED);
        }
    }

    #[tokio::test]
    async fn socks5_refuses_bad_atyp_and_auth() {
        let (r, out) = run_handshake(vec![5, 1, 0, 5, 1, 0, 9, 0, 0]).await;
        assert!(r.is_err());
        assert_eq!(out[3], REP_ATYP_UNSUPPORTED);
        // Seule l'authentification par mot de passe proposée : refus (0xFF).
        let (r, out) = run_handshake(vec![5, 1, 2]).await;
        assert!(r.is_err());
        assert_eq!(out, vec![5, 0xFF]);
        // Mauvaise version, domaine vide, troncature.
        assert!(run_handshake(vec![4, 1, 0]).await.0.is_err());
        let (r, out) = run_handshake(vec![5, 1, 0, 5, 1, 0, 3, 0, 0, 80]).await;
        assert!(r.is_err());
        assert_eq!(out[3], REP_GENERAL);
        assert!(run_handshake(vec![5, 1, 0, 5, 1, 0, 1, 1]).await.0.is_err());
    }

    #[test]
    fn bind_parsing_defaults_to_loopback() {
        assert_eq!(parse_bind("").unwrap(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(
            parse_bind("localhost").unwrap(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_eq!(
            parse_bind("[::1]").unwrap(),
            IpAddr::V6(Ipv6Addr::LOCALHOST)
        );
        assert!(parse_bind("0.0.0.0").unwrap().is_unspecified());
        assert!(parse_bind("exemple.org").is_err());
    }

    #[test]
    fn def_validation() {
        let mut d = TunnelDef {
            id: "t".into(),
            kind: TunnelKind::Local,
            bind_addr: String::new(),
            bind_port: 0,
            dest_host: String::new(),
            dest_port: 80,
        };
        assert!(d.validate().is_err());
        d.dest_host = "h".into();
        assert!(d.validate().is_ok());
        d.kind = TunnelKind::Dynamic;
        d.dest_host.clear();
        d.dest_port = 0;
        assert!(d.validate().is_ok());
        assert!("bogus".parse::<TunnelKind>().is_err());
    }
}
