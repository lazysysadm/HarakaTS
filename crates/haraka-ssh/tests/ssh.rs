//! Tests M3 contre le serveur russh in-process : auth (password, clé ed25519,
//! clé chiffrée, keyboard-interactive/OTP, agent), host keys (TOFU, match,
//! changement bloquant/remplacement), jump hosts, resize, reconnexion.

mod common;

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use common::{TestServer, TestServerSpec};
use haraka_core::knownhosts::{self, KnownHostStatus};
use haraka_ssh::{
    AuthMaterial, HopConfig, HostKeyDecision, OpenRequest, ReconnectPolicy, SessionEvent,
    SessionManager,
};
use russh::keys::{Algorithm, PrivateKey};
use secrecy::SecretString;
use tokio::sync::mpsc;

fn secret(s: &str) -> SecretString {
    SecretString::from(s.to_string())
}

struct Counters {
    auth_prompts: Arc<AtomicU32>,
    hostkey_prompts: Arc<AtomicU32>,
    last_kind: Arc<StdMutex<Option<String>>>,
}

fn spawn_responder(
    manager: Arc<SessionManager>,
    mut rx: mpsc::UnboundedReceiver<SessionEvent>,
    accept_hostkeys: bool,
    otp_answer: &'static str,
) -> Counters {
    let auth_prompts = Arc::new(AtomicU32::new(0));
    let hostkey_prompts = Arc::new(AtomicU32::new(0));
    let last_kind = Arc::new(StdMutex::new(None));
    let (a, h, k) = (
        auth_prompts.clone(),
        hostkey_prompts.clone(),
        last_kind.clone(),
    );
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                SessionEvent::AuthPrompt { prompt_id, .. } => {
                    a.fetch_add(1, Ordering::SeqCst);
                    let _ = manager
                        .respond_auth(&prompt_id, vec![otp_answer.to_string()])
                        .await;
                }
                SessionEvent::HostKeyPrompt {
                    prompt_id, kind, ..
                } => {
                    h.fetch_add(1, Ordering::SeqCst);
                    *k.lock().unwrap() = Some(kind);
                    let _ = manager
                        .respond_hostkey(
                            &prompt_id,
                            HostKeyDecision {
                                accept: accept_hostkeys,
                                remember: accept_hostkeys,
                            },
                        )
                        .await;
                }
                _ => {}
            }
        }
    });
    Counters {
        auth_prompts,
        hostkey_prompts,
        last_kind,
    }
}

fn request(
    id: &str,
    addr: std::net::SocketAddr,
    materials: Vec<AuthMaterial>,
    known_hosts: std::path::PathBuf,
    reconnect: bool,
) -> OpenRequest {
    OpenRequest {
        session_id: id.to_string(),
        hops: vec![],
        target: HopConfig {
            host: addr.ip().to_string(),
            port: addr.port(),
            user: "test".to_string(),
            materials,
        },
        cols: 80,
        rows: 24,
        keepalive_s: 0,
        reconnect: ReconnectPolicy {
            enabled: reconnect,
            max_attempts: 3,
            backoff_max_s: 1,
        },
        known_hosts,
    }
}

async fn wait_state(manager: &SessionManager, id: &str, want: &str, secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if manager.states().await.get(id).map(String::as_str) == Some(want) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn echo_check(
    manager: &SessionManager,
    out: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    id: &str,
    marker: &str,
) {
    manager.write(id, marker.as_bytes().to_vec()).await.unwrap();
    let mut buf = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), out.recv()).await {
            Ok(Some(chunk)) => {
                buf.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&buf).contains(marker) {
                    return;
                }
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    panic!("écho non reçu : {marker}");
}

#[tokio::test]
async fn password_auth_and_echo() {
    let server = TestServer::start(TestServerSpec::password("pw-test")).await;
    let tmp = tempfile::tempdir().unwrap();
    let (manager, rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let _c = spawn_responder(manager.clone(), rx, true, "");
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            request(
                "s1",
                server.addr,
                vec![AuthMaterial::Password(secret("pw-test"))],
                tmp.path().join("known_hosts"),
                false,
            ),
            out_tx,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s1", "connected", 10).await);
    echo_check(&manager, &mut out_rx, "s1", "echo-password\r").await;
    manager.close("s1").await.unwrap();
}

#[tokio::test]
async fn wrong_password_fails() {
    let server = TestServer::start(TestServerSpec::password("pw-test")).await;
    let tmp = tempfile::tempdir().unwrap();
    let (manager, rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let _c = spawn_responder(manager.clone(), rx, true, "");
    let (out_tx, _out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            request(
                "s1",
                server.addr,
                vec![AuthMaterial::Password(secret("mauvais"))],
                tmp.path().join("known_hosts"),
                false,
            ),
            out_tx,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s1", "closed", 10).await);
}

#[tokio::test]
async fn pubkey_auth_ed25519() {
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let mut spec = TestServerSpec::password("unused");
    spec.authorized_key = Some(key.public_key().clone());
    let server = TestServer::start(spec).await;
    let tmp = tempfile::tempdir().unwrap();
    let (manager, rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let _c = spawn_responder(manager.clone(), rx, true, "");
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let pem = key
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .unwrap();
    manager
        .open(
            request(
                "s1",
                server.addr,
                vec![AuthMaterial::Key {
                    pem: secret(&pem),
                    passphrase: None,
                }],
                tmp.path().join("known_hosts"),
                false,
            ),
            out_tx,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s1", "connected", 10).await);
    echo_check(&manager, &mut out_rx, "s1", "echo-key\r").await;
}

#[tokio::test]
async fn encrypted_key_auth_and_wrong_passphrase() {
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let mut spec = TestServerSpec::password("unused");
    spec.authorized_key = Some(key.public_key().clone());
    let server = TestServer::start(spec).await;
    let mut buf = Vec::new();
    russh::keys::encode_pkcs8_pem_encrypted(&key, b"pp-test", 4, &mut buf).unwrap();
    let pem = String::from_utf8(buf).unwrap();
    let tmp = tempfile::tempdir().unwrap();

    // Bonne passphrase → connecté.
    let (manager, rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let _c = spawn_responder(manager.clone(), rx, true, "");
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            request(
                "s1",
                server.addr,
                vec![AuthMaterial::Key {
                    pem: secret(&pem),
                    passphrase: Some(secret("pp-test")),
                }],
                tmp.path().join("known_hosts"),
                false,
            ),
            out_tx,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s1", "connected", 10).await);
    echo_check(&manager, &mut out_rx, "s1", "echo-enc\r").await;
    manager.close("s1").await.unwrap();

    // Mauvaise passphrase → échec propre.
    let (manager2, rx2) = SessionManager::new();
    let manager2 = Arc::new(manager2);
    let _c2 = spawn_responder(manager2.clone(), rx2, true, "");
    let (out_tx2, _out_rx2) = mpsc::unbounded_channel();
    manager2
        .open(
            request(
                "s2",
                server.addr,
                vec![AuthMaterial::Key {
                    pem: secret(&pem),
                    passphrase: Some(secret("mauvaise")),
                }],
                tmp.path().join("known_hosts"),
                false,
            ),
            out_tx2,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager2, "s2", "closed", 10).await);
}

#[tokio::test]
async fn keyboard_interactive_otp() {
    let mut spec = TestServerSpec::password("unused");
    spec.otp = Some("123456".to_string());
    let server = TestServer::start(spec).await;
    let tmp = tempfile::tempdir().unwrap();
    let (manager, rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let counters = spawn_responder(manager.clone(), rx, true, "123456");
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            request(
                "s1",
                server.addr,
                vec![AuthMaterial::KeyboardInteractive { prefill: None }],
                tmp.path().join("known_hosts"),
                false,
            ),
            out_tx,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s1", "connected", 10).await);
    assert!(counters.auth_prompts.load(Ordering::SeqCst) >= 1);
    echo_check(&manager, &mut out_rx, "s1", "echo-otp\r").await;
}

#[tokio::test]
async fn hostkey_tofu_then_match() {
    let server = TestServer::start(TestServerSpec::password("pw")).await;
    let tmp = tempfile::tempdir().unwrap();
    let kh = tmp.path().join("known_hosts");
    let (manager, rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let counters = spawn_responder(manager.clone(), rx, true, "");
    let (out_tx, _out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            request(
                "s1",
                server.addr,
                vec![AuthMaterial::Password(secret("pw"))],
                kh.clone(),
                false,
            ),
            out_tx,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s1", "connected", 10).await);
    assert_eq!(counters.hostkey_prompts.load(Ordering::SeqCst), 1);
    assert!(kh.exists());
    manager.close("s1").await.unwrap();

    // Seconde connexion : la clé est connue, aucun prompt.
    let (out_tx2, _out_rx2) = mpsc::unbounded_channel();
    manager
        .open(
            request(
                "s2",
                server.addr,
                vec![AuthMaterial::Password(secret("pw"))],
                kh.clone(),
                false,
            ),
            out_tx2,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s2", "connected", 10).await);
    assert_eq!(counters.hostkey_prompts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn hostkey_changed_blocks_then_replace() {
    let server = TestServer::start(TestServerSpec::password("pw")).await;
    let tmp = tempfile::tempdir().unwrap();
    let kh = tmp.path().join("known_hosts");
    // Enregistre une AUTRE clé pour ce host:port → changement détecté.
    let other = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let other_b64 = B64.encode(other.public_key().to_bytes().unwrap());
    knownhosts::learn(
        &kh,
        &server.addr.ip().to_string(),
        server.addr.port(),
        "ssh-ed25519",
        &other_b64,
    )
    .unwrap();

    // Refus → connexion bloquée, fichier inchangé.
    let (manager, rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let counters = spawn_responder(manager.clone(), rx, false, "");
    let (out_tx, _out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            request(
                "s1",
                server.addr,
                vec![AuthMaterial::Password(secret("pw"))],
                kh.clone(),
                false,
            ),
            out_tx,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s1", "closed", 10).await);
    assert_eq!(
        counters.last_kind.lock().unwrap().as_deref(),
        Some("changed")
    );
    let entries = knownhosts::list(&kh).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key_b64, other_b64);

    // Même un répondeur qui « accepte tout » ne débloque pas : blocage dur, fichier inchangé.
    let (manager_b, rx_b) = SessionManager::new();
    let manager_b = Arc::new(manager_b);
    let _cb = spawn_responder(manager_b.clone(), rx_b, true, "");
    let (out_tx_b, _out_rx_b) = mpsc::unbounded_channel();
    manager_b
        .open(
            request(
                "s1b",
                server.addr,
                vec![AuthMaterial::Password(secret("pw"))],
                kh.clone(),
                false,
            ),
            out_tx_b,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager_b, "s1b", "closed", 10).await);
    assert_eq!(knownhosts::list(&kh).unwrap()[0].key_b64, other_b64);

    // Remplacement VOLONTAIRE : oubli explicite de l'ancienne entrée, puis TOFU normal.
    let old = knownhosts::list(&kh).unwrap().remove(0);
    assert_eq!(
        knownhosts::forget(&kh, &old.host_field, &old.fingerprint).unwrap(),
        1
    );
    assert!(knownhosts::list(&kh).unwrap().is_empty());
    let (manager2, rx2) = SessionManager::new();
    let manager2 = Arc::new(manager2);
    let _c2 = spawn_responder(manager2.clone(), rx2, true, "");
    let (out_tx2, _out_rx2) = mpsc::unbounded_channel();
    manager2
        .open(
            request(
                "s2",
                server.addr,
                vec![AuthMaterial::Password(secret("pw"))],
                kh.clone(),
                false,
            ),
            out_tx2,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager2, "s2", "connected", 10).await);
    // La nouvelle clé du serveur est désormais Match ; une seule entrée.
    assert_eq!(
        knownhosts::check(
            &kh,
            &server.addr.ip().to_string(),
            server.addr.port(),
            "ssh-ed25519",
            &server.host_key_b64
        )
        .unwrap(),
        KnownHostStatus::Match
    );
    assert_eq!(knownhosts::list(&kh).unwrap().len(), 1);
}

#[tokio::test]
async fn jump_host_chain() {
    let bastion = TestServer::start(TestServerSpec::password("jump-pw")).await;
    let target = TestServer::start(TestServerSpec::password("target-pw")).await;
    let tmp = tempfile::tempdir().unwrap();
    let (manager, mut rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let m = manager.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            eprintln!("JUMP EVENT: {ev:?}");
            match &ev {
                SessionEvent::AuthPrompt { prompt_id, .. } => {
                    let _ = m.respond_auth(prompt_id, vec![]).await;
                }
                SessionEvent::HostKeyPrompt { prompt_id, .. } => {
                    let _ = m
                        .respond_hostkey(
                            prompt_id,
                            HostKeyDecision {
                                accept: true,
                                remember: true,
                            },
                        )
                        .await;
                }
                _ => {}
            }
        }
    });
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let mut req = request(
        "s1",
        target.addr,
        vec![AuthMaterial::Password(secret("target-pw"))],
        tmp.path().join("known_hosts"),
        false,
    );
    req.hops = vec![HopConfig {
        host: bastion.addr.ip().to_string(),
        port: bastion.addr.port(),
        user: "test".to_string(),
        materials: vec![AuthMaterial::Password(secret("jump-pw"))],
    }];
    manager.open(req, out_tx).await.unwrap();
    let ok = wait_state(&manager, "s1", "connected", 15).await;
    if !ok {
        eprintln!("JUMP STATES: {:?}", manager.states().await);
    }
    assert!(ok);
    echo_check(&manager, &mut out_rx, "s1", "echo-jump\r").await;
}

#[tokio::test]
async fn resize_propagates() {
    let server = TestServer::start(TestServerSpec::password("pw")).await;
    let tmp = tempfile::tempdir().unwrap();
    let (manager, rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let _c = spawn_responder(manager.clone(), rx, true, "");
    let (out_tx, _out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            request(
                "s1",
                server.addr,
                vec![AuthMaterial::Password(secret("pw"))],
                tmp.path().join("known_hosts"),
                false,
            ),
            out_tx,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s1", "connected", 10).await);
    manager.resize("s1", 120, 40).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if server.window.0.load(Ordering::SeqCst) == 120
            && server.window.1.load(Ordering::SeqCst) == 40
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("resize non propagé");
}

/// Rafale de resizes : la DERNIÈRE taille doit gagner côté serveur, et
/// l'écriture doit rester réactive (régression gel htop au redimensionnement).
#[tokio::test]
async fn resize_burst_last_wins_and_write_stays_live() {
    let server = TestServer::start(TestServerSpec::password("pw")).await;
    let tmp = tempfile::tempdir().unwrap();
    let (manager, rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let _c = spawn_responder(manager.clone(), rx, true, "");
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            request(
                "s1",
                server.addr,
                vec![AuthMaterial::Password(secret("pw"))],
                tmp.path().join("known_hosts"),
                false,
            ),
            out_tx,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s1", "connected", 10).await);

    for i in 0..50u32 {
        manager.resize("s1", 80 + i, 24 + i).await.unwrap();
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut last_ok = false;
    while tokio::time::Instant::now() < deadline {
        if server.window.0.load(Ordering::SeqCst) == 129
            && server.window.1.load(Ordering::SeqCst) == 73
        {
            last_ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        last_ok,
        "dernière taille non appliquée : {:?}",
        (
            server.window.0.load(Ordering::SeqCst),
            server.window.1.load(Ordering::SeqCst)
        )
    );
    echo_check(&manager, &mut out_rx, "s1", "echo-after-burst\r").await;
}

#[tokio::test]
async fn reconnect_after_server_drop() {
    let mut spec = TestServerSpec::password("pw");
    spec.close_after_ms = Some(400);
    let server = TestServer::start(spec).await;
    let tmp = tempfile::tempdir().unwrap();
    let (manager, rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let _c = spawn_responder(manager.clone(), rx, true, "");
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            request(
                "s1",
                server.addr,
                vec![AuthMaterial::Password(secret("pw"))],
                tmp.path().join("known_hosts"),
                true,
            ),
            out_tx,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s1", "connected", 10).await);
    // Le serveur coupe après 400 ms ; le client doit se reconnecter seul.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(wait_state(&manager, "s1", "connected", 15).await);
    echo_check(&manager, &mut out_rx, "s1", "echo-reconnect\r").await;
}

#[tokio::test]
async fn agent_unavailable_fails_gracefully() {
    if std::env::var("SSH_AUTH_SOCK").is_ok() {
        eprintln!("SSH_AUTH_SOCK présent — test ignoré");
        return;
    }
    let server = TestServer::start(TestServerSpec::password("pw")).await;
    let tmp = tempfile::tempdir().unwrap();
    let (manager, rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let _c = spawn_responder(manager.clone(), rx, true, "");
    let (out_tx, _out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            request(
                "s1",
                server.addr,
                vec![AuthMaterial::Agent],
                tmp.path().join("known_hosts"),
                false,
            ),
            out_tx,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s1", "closed", 10).await);
}

/// Régression gel htop : la file interne du canal russh (100 messages) n'était
/// pas drainée ; au ~100e message la boucle de session se bloquait. 400 échos
/// d'un octet (≥ 400 messages) doivent TOUS revenir, puis un resize doit passer.
#[tokio::test]
async fn many_small_messages_do_not_stall_session() {
    let server = TestServer::start(TestServerSpec::password("pw")).await;
    let tmp = tempfile::tempdir().unwrap();
    let (manager, rx) = SessionManager::new();
    let manager = Arc::new(manager);
    let _c = spawn_responder(manager.clone(), rx, true, "");
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            request(
                "s1",
                server.addr,
                vec![AuthMaterial::Password(secret("pw"))],
                tmp.path().join("known_hosts"),
                false,
            ),
            out_tx,
        )
        .await
        .unwrap();
    assert!(wait_state(&manager, "s1", "connected", 10).await);
    // Vide la bannière.
    tokio::time::sleep(Duration::from_millis(300)).await;
    while out_rx.try_recv().is_ok() {}

    const N: usize = 400;
    for _ in 0..N {
        manager.write("s1", b"x".to_vec()).await.unwrap();
    }
    let mut got = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while got < N && tokio::time::Instant::now() < deadline {
        if let Ok(Some(chunk)) =
            tokio::time::timeout(Duration::from_millis(500), out_rx.recv()).await
        {
            got += chunk.len();
        }
    }
    assert_eq!(
        got, N,
        "sortie bloquée après ~100 messages (file non drainée)"
    );
    manager.resize("s1", 100, 30).await.unwrap();
}
