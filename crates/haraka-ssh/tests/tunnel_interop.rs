//! Interop tunnels SSH (M7) contre un vrai OpenSSH (conteneur de dev / CI).
//! Activé par `HARAKA_SSHD_TEST=1` — voir `tests/sshd/setup.sh`.

use std::sync::Arc;
use std::time::Duration;

use haraka_ssh::{
    AuthMaterial, HopConfig, HostKeyDecision, OpenRequest, ReconnectPolicy, SessionEvent,
    SessionManager, TunnelDef, TunnelInfo, TunnelKind, TunnelManager, TunnelState,
};
use secrecy::SecretString;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

fn enabled() -> bool {
    if std::env::var("HARAKA_SSHD_TEST").ok().as_deref() != Some("1") {
        eprintln!("interop tunnels ignorée (HARAKA_SSHD_TEST != 1) — voir tests/sshd/setup.sh");
        return false;
    }
    true
}

fn sshd_port() -> u16 {
    std::env::var("HARAKA_TEST_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(2222)
}

struct Env {
    sessions: Arc<SessionManager>,
    tunnels: TunnelManager,
    _tmp: tempfile::TempDir,
}

async fn connect(sid: &str) -> Env {
    let key_path = std::env::var("HARAKA_TEST_KEY").expect("HARAKA_TEST_KEY requis");
    let user = std::env::var("HARAKA_TEST_USER").unwrap_or_else(|_| "harakats-test".into());
    let pem = std::fs::read_to_string(&key_path).expect("clé de test lisible");
    let tmp = tempfile::tempdir().unwrap();
    let (manager, mut rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let m = manager.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let SessionEvent::HostKeyPrompt { prompt_id, .. } = ev {
                let _ = m
                    .respond_hostkey(
                        &prompt_id,
                        HostKeyDecision {
                            accept: true,
                            remember: true,
                        },
                    )
                    .await;
            }
        }
    });
    let (out_tx, _out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            OpenRequest {
                session_id: sid.into(),
                hops: vec![],
                target: HopConfig {
                    host: "127.0.0.1".into(),
                    port: sshd_port(),
                    user,
                    materials: vec![AuthMaterial::Key {
                        pem: SecretString::from(pem),
                        passphrase: None,
                    }],
                },
                cols: 80,
                rows: 24,
                keepalive_s: 0,
                reconnect: ReconnectPolicy {
                    enabled: false,
                    max_attempts: 0,
                    backoff_max_s: 1,
                },
                known_hosts: tmp.path().join("known_hosts"),
            },
            out_tx,
        )
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if manager.states().await.get(sid).map(String::as_str) == Some("connected") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "connexion non établie"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (tunnels, _events) = TunnelManager::new(manager.clone());
    // Les événements non lus s'accumulent : sans importance pour les tests.
    std::mem::forget(_events);
    Env {
        sessions: manager,
        tunnels,
        _tmp: tmp,
    }
}

fn def(id: &str, kind: TunnelKind, bind_port: u16, dest_host: &str, dest_port: u16) -> TunnelDef {
    TunnelDef {
        id: id.into(),
        kind,
        bind_addr: String::new(),
        bind_port,
        dest_host: dest_host.into(),
        dest_port,
    }
}

/// Serveur d'écho local (127.0.0.1, port éphémère).
async fn echo_server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    port
}

async fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

async fn banner(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut b = [0u8; 1];
    let read = async {
        while stream.read_exact(&mut b).await.is_ok() {
            buf.push(b[0]);
            if b[0] == b'\n' {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .expect("bannière reçue à temps");
    String::from_utf8_lossy(&buf).to_string()
}

async fn echo_roundtrip(port: u16, payload: &[u8]) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(payload).await.unwrap();
    let mut got = vec![0u8; payload.len()];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut got))
        .await
        .expect("écho à temps")
        .unwrap();
    assert_eq!(got, payload);
}

async fn assert_port_free(port: u16) {
    for _ in 0..50 {
        if TcpListener::bind(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("le port {port} n'a pas été libéré");
}

async fn assert_refused(port: u16) {
    for _ in 0..50 {
        let r = tokio::time::timeout(
            Duration::from_secs(2),
            TcpStream::connect(("127.0.0.1", port)),
        )
        .await;
        if matches!(r, Ok(Err(_))) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("le port {port} accepte encore des connexions");
}

fn assert_listening(info: &TunnelInfo) {
    assert_eq!(info.state, TunnelState::Listening, "{info:?}");
    assert_ne!(info.bind_port, 0);
}

// ------------------------------------------------------------------ SOCKS5

/// Client SOCKS5 minimal : retourne le flux et le code de réponse.
async fn socks_connect(proxy: u16, cmd: u8, addr: &[u8], atyp: u8, port: u16) -> (TcpStream, u8) {
    let mut s = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut m = [0u8; 2];
    s.read_exact(&mut m).await.unwrap();
    assert_eq!(m, [5, 0]);
    let mut req = vec![5, cmd, 0, atyp];
    if atyp == 3 {
        req.push(addr.len() as u8);
    }
    req.extend_from_slice(addr);
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep).await.unwrap();
    assert_eq!(rep[0], 5);
    (s, rep[1])
}

// ------------------------------------------------------------------- tests

#[tokio::test]
async fn local_forward_reads_sshd_banner() {
    if !enabled() {
        return;
    }
    let env = connect("tun-local").await;
    let info = env
        .tunnels
        .start(
            "tun-local",
            def("l1", TunnelKind::Local, 0, "127.0.0.1", sshd_port()),
        )
        .await
        .unwrap();
    assert_listening(&info);
    assert_eq!(info.bind_addr, "127.0.0.1", "bind par défaut = loopback");
    let mut s = TcpStream::connect(("127.0.0.1", info.bind_port))
        .await
        .unwrap();
    assert!(banner(&mut s).await.starts_with("SSH-2.0-"));
    drop(s);
    let live = env.tunnels.list();
    assert_eq!(live.len(), 1);
    assert!(
        live[0].bytes_in > 0,
        "compteur d'octets entrants : {live:?}"
    );
    // Démarrer deux fois = idempotent.
    let again = env
        .tunnels
        .start(
            "tun-local",
            def("l1", TunnelKind::Local, 0, "127.0.0.1", sshd_port()),
        )
        .await
        .unwrap();
    assert_eq!(again.bind_port, info.bind_port);
    assert_eq!(env.tunnels.list().len(), 1);
    env.tunnels.stop_all("tun-local").await;
}

#[tokio::test]
async fn local_forward_validation() {
    if !enabled() {
        return;
    }
    let env = connect("tun-valid").await;
    assert!(
        env.tunnels
            .start("tun-valid", def("x", TunnelKind::Local, 0, "", 22))
            .await
            .is_err()
    );
    assert!(
        env.tunnels
            .start("absent", def("x", TunnelKind::Dynamic, 0, "", 0))
            .await
            .is_err()
    );
    let mut d = def("y", TunnelKind::Local, 0, "127.0.0.1", 22);
    d.bind_addr = "pas une adresse".into();
    let info = env.tunnels.start("tun-valid", d).await.unwrap();
    assert_eq!(info.state, TunnelState::Error);
    assert!(env.tunnels.list().len() == 1);
    env.tunnels.stop("tun-valid", "y").await.unwrap();
    assert!(env.tunnels.list().is_empty());
}

#[tokio::test]
async fn dynamic_socks5() {
    if !enabled() {
        return;
    }
    let env = connect("tun-socks").await;
    let echo = echo_server().await;
    let info = env
        .tunnels
        .start("tun-socks", def("d1", TunnelKind::Dynamic, 0, "", 0))
        .await
        .unwrap();
    assert_listening(&info);
    let proxy = info.bind_port;

    // Nom de domaine -> écho.
    let (mut s, rep) = socks_connect(proxy, 1, b"localhost", 3, echo).await;
    assert_eq!(rep, 0);
    s.write_all(b"bonjour").await.unwrap();
    let mut got = [0u8; 7];
    s.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"bonjour");

    // IPv4 -> bannière sshd.
    let (mut s, rep) = socks_connect(proxy, 1, &[127, 0, 0, 1], 1, sshd_port()).await;
    assert_eq!(rep, 0);
    assert!(banner(&mut s).await.starts_with("SSH-2.0-"));

    // IPv6 (::1) : la connexion est tentée côté serveur ; réponse valide dans tous les cas.
    let (_s, rep) = socks_connect(
        proxy,
        1,
        &[0; 15].iter().chain(&[1]).copied().collect::<Vec<_>>(),
        4,
        sshd_port(),
    )
    .await;
    assert!(rep == 0 || rep == 5 || rep == 1, "réponse IPv6 : {rep}");

    // BIND et UDP ASSOCIATE : commande non prise en charge.
    for cmd in [2u8, 3] {
        let (_s, rep) = socks_connect(proxy, cmd, &[127, 0, 0, 1], 1, 80).await;
        assert_eq!(rep, 7, "commande {cmd}");
    }
    // Type d'adresse inconnu.
    let mut raw = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    raw.write_all(&[5, 1, 0]).await.unwrap();
    let mut m = [0u8; 2];
    raw.read_exact(&mut m).await.unwrap();
    raw.write_all(&[5, 1, 0, 9, 0, 0]).await.unwrap();
    let mut rep = [0u8; 10];
    raw.read_exact(&mut rep).await.unwrap();
    assert_eq!(rep[1], 8);

    // Destination fermée : connexion refusée (5).
    let closed = free_port().await;
    let (_s, rep) = socks_connect(proxy, 1, &[127, 0, 0, 1], 1, closed).await;
    assert_eq!(rep, 5);
    env.tunnels.stop_all("tun-socks").await;
    assert_port_free(proxy).await;
}

#[tokio::test]
async fn remote_forward_and_cancel() {
    if !enabled() {
        return;
    }
    let env = connect("tun-remote").await;
    let echo = echo_server().await;
    // Port 0 : choisi par le serveur et rapporté.
    let info = env
        .tunnels
        .start(
            "tun-remote",
            def("r0", TunnelKind::Remote, 0, "127.0.0.1", echo),
        )
        .await
        .unwrap();
    assert_listening(&info);
    echo_roundtrip(info.bind_port, b"via le serveur").await;
    let (a, b) = (
        echo_roundtrip(info.bind_port, b"un"),
        echo_roundtrip(info.bind_port, b"deux"),
    );
    tokio::join!(a, b);

    // Port fixe.
    let fixed = free_port().await;
    let info2 = env
        .tunnels
        .start(
            "tun-remote",
            def("r1", TunnelKind::Remote, fixed, "127.0.0.1", echo),
        )
        .await
        .unwrap();
    assert_eq!(info2.bind_port, fixed, "{info2:?}");
    echo_roundtrip(fixed, b"port fixe").await;

    env.tunnels.stop("tun-remote", "r0").await.unwrap();
    assert_refused(info.bind_port).await;
    env.tunnels.stop("tun-remote", "r1").await.unwrap();
    assert_refused(fixed).await;
    // Le port distant peut être redemandé : l'annulation a bien eu lieu.
    let info3 = env
        .tunnels
        .start(
            "tun-remote",
            def("r2", TunnelKind::Remote, fixed, "127.0.0.1", echo),
        )
        .await
        .unwrap();
    assert_listening(&info3);
    echo_roundtrip(fixed, b"reprise").await;
    env.tunnels.stop_all("tun-remote").await;
    assert_refused(fixed).await;
}

#[tokio::test]
async fn remote_forward_conflict_is_error_state() {
    if !enabled() {
        return;
    }
    let env = connect("tun-remote2").await;
    let echo = echo_server().await;
    let fixed = free_port().await;
    let a = env
        .tunnels
        .start(
            "tun-remote2",
            def("a", TunnelKind::Remote, fixed, "127.0.0.1", echo),
        )
        .await
        .unwrap();
    assert_listening(&a);
    let b = env
        .tunnels
        .start(
            "tun-remote2",
            def("b", TunnelKind::Remote, fixed, "127.0.0.1", echo),
        )
        .await
        .unwrap();
    assert_eq!(b.state, TunnelState::Error, "{b:?}");
    assert!(b.error.is_some());
    // Le tunnel « a » n'est pas affecté.
    echo_roundtrip(fixed, b"toujours la").await;
    env.tunnels.stop_all("tun-remote2").await;
}

#[tokio::test]
async fn stop_releases_ports_and_bind_errors() {
    if !enabled() {
        return;
    }
    let env = connect("tun-stop").await;
    let echo = echo_server().await;
    let a = env
        .tunnels
        .start(
            "tun-stop",
            def("a", TunnelKind::Local, 0, "127.0.0.1", echo),
        )
        .await
        .unwrap();
    let b = env
        .tunnels
        .start("tun-stop", def("b", TunnelKind::Dynamic, 0, "", 0))
        .await
        .unwrap();
    // Port déjà utilisé : état error, pas de panique.
    let c = env
        .tunnels
        .start(
            "tun-stop",
            def("c", TunnelKind::Local, a.bind_port, "127.0.0.1", echo),
        )
        .await
        .unwrap();
    assert_eq!(c.state, TunnelState::Error);
    assert!(
        c.error.as_deref().unwrap().contains("déjà utilisé"),
        "{c:?}"
    );
    assert_eq!(env.tunnels.list().len(), 3);
    // Un tunnel en erreur est relancé proprement après libération.
    env.tunnels.stop("tun-stop", "a").await.unwrap();
    assert_port_free(a.bind_port).await;
    // Connexion ouverte pendant l'arrêt : fermée.
    let mut open = TcpStream::connect(("127.0.0.1", b.bind_port))
        .await
        .unwrap();
    let n = env.tunnels.stop_all("tun-stop").await;
    assert_eq!(n, 2);
    assert!(env.tunnels.list().is_empty());
    assert_port_free(b.bind_port).await;
    let mut buf = [0u8; 8];
    let r = tokio::time::timeout(Duration::from_secs(5), open.read(&mut buf))
        .await
        .expect("connexion fermée à l'arrêt");
    assert!(matches!(r, Ok(0) | Err(_)));
    assert!(env.tunnels.stop("tun-stop", "a").await.is_err());
}

#[tokio::test]
async fn session_close_tears_everything_down() {
    if !enabled() {
        return;
    }
    let env = connect("tun-close").await;
    let echo = echo_server().await;
    let l = env
        .tunnels
        .start(
            "tun-close",
            def("l", TunnelKind::Local, 0, "127.0.0.1", echo),
        )
        .await
        .unwrap();
    let d = env
        .tunnels
        .start("tun-close", def("d", TunnelKind::Dynamic, 0, "", 0))
        .await
        .unwrap();
    let fixed = free_port().await;
    let r = env
        .tunnels
        .start(
            "tun-close",
            def("r", TunnelKind::Remote, fixed, "127.0.0.1", echo),
        )
        .await
        .unwrap();
    assert_listening(&r);
    env.sessions.close("tun-close").await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !env.tunnels.list().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "tunnels non arrêtés : {:?}",
            env.tunnels.list()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_port_free(l.bind_port).await;
    assert_port_free(d.bind_port).await;
    assert_refused(fixed).await;
    // Plus de session : démarrage refusé.
    assert!(
        env.tunnels
            .start("tun-close", def("n", TunnelKind::Dynamic, 0, "", 0))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn hundred_concurrent_connections() {
    if !enabled() {
        return;
    }
    let env = connect("tun-conc").await;
    let echo = echo_server().await;
    let info = env
        .tunnels
        .start(
            "tun-conc",
            def("l", TunnelKind::Local, 0, "127.0.0.1", echo),
        )
        .await
        .unwrap();
    let port = info.bind_port;
    let mut tasks = Vec::new();
    for i in 0..100u32 {
        tasks.push(tokio::spawn(async move {
            let payload: Vec<u8> = (0..4096u32).map(|j| ((i * 31 + j) % 251) as u8).collect();
            echo_roundtrip(port, &payload).await;
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let live = env.tunnels.list();
    assert!(live[0].bytes_out >= 100 * 4096, "{live:?}");
    env.tunnels.stop_all("tun-conc").await;
}

#[tokio::test]
async fn large_transfer_integrity() {
    if !enabled() {
        return;
    }
    const SIZE: usize = 20 * 1024 * 1024;
    let env = connect("tun-big").await;
    let echo = echo_server().await;
    let info = env
        .tunnels
        .start("tun-big", def("l", TunnelKind::Local, 0, "127.0.0.1", echo))
        .await
        .unwrap();
    let data: Vec<u8> = (0..SIZE)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect();
    let expected = Sha256::digest(&data);
    let s = TcpStream::connect(("127.0.0.1", info.bind_port))
        .await
        .unwrap();
    let (mut r, mut w) = s.into_split();
    let writer = tokio::spawn(async move {
        w.write_all(&data).await.unwrap();
        w
    });
    let mut hasher = Sha256::new();
    let mut total = 0usize;
    let mut buf = vec![0u8; 64 * 1024];
    while total < SIZE {
        let n = tokio::time::timeout(Duration::from_secs(30), r.read(&mut buf))
            .await
            .expect("lecture à temps")
            .unwrap();
        assert!(n > 0, "flux fermé après {total} octets");
        hasher.update(&buf[..n]);
        total += n;
    }
    let _w = writer.await.unwrap();
    assert_eq!(total, SIZE);
    assert_eq!(hasher.finalize(), expected);
    env.tunnels.stop_all("tun-big").await;
}
