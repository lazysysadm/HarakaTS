//! Interop avec un vrai serveur OpenSSH (conteneur de dev / CI).
//! Activé par `HARAKA_SSHD_TEST=1` — voir `tests/sshd/setup.sh`.
//! Sans la variable, le test est ignoré proprement (pas de dépendance réseau).

use std::sync::Arc;
use std::time::Duration;

use haraka_ssh::{
    AuthMaterial, HopConfig, HostKeyDecision, OpenRequest, ReconnectPolicy, SessionEvent,
    SessionManager,
};
use secrecy::SecretString;
use tokio::sync::mpsc;

#[tokio::test]
async fn openssh_key_auth_shell_echo() {
    if std::env::var("HARAKA_SSHD_TEST").ok().as_deref() != Some("1") {
        eprintln!("interop ignorée (HARAKA_SSHD_TEST != 1) — voir tests/sshd/setup.sh");
        return;
    }
    let key_path = std::env::var("HARAKA_TEST_KEY").expect("HARAKA_TEST_KEY requis");
    let user = std::env::var("HARAKA_TEST_USER").unwrap_or_else(|_| "harakats-test".into());
    let port: u16 = std::env::var("HARAKA_TEST_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(2222);
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

    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            OpenRequest {
                session_id: "interop".into(),
                hops: vec![],
                target: HopConfig {
                    host: "127.0.0.1".into(),
                    port,
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
    let mut connected = false;
    while tokio::time::Instant::now() < deadline {
        if manager.states().await.get("interop").map(String::as_str) == Some("connected") {
            connected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(connected, "connexion openssh non établie");

    manager
        .write("interop", b"echo interop-ok\r".to_vec())
        .await
        .unwrap();
    let mut buf = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(300), out_rx.recv()).await {
            Ok(Some(chunk)) => {
                buf.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&buf).contains("interop-ok") {
                    manager.close("interop").await.unwrap();
                    return;
                }
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    panic!("écho openssh non reçu");
}

async fn wait_connected(manager: &SessionManager, id: &str) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if manager.states().await.get(id).map(String::as_str) == Some("connected") {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn read_until(
    out_rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    needle: &str,
    secs: u64,
) -> bool {
    let mut buf = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(300), out_rx.recv()).await {
            Ok(Some(chunk)) => {
                buf.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&buf).contains(needle) {
                    return true;
                }
            }
            Ok(None) => return false,
            Err(_) => continue,
        }
    }
    false
}

/// Resize de bout en bout contre un vrai sshd : `stty size` doit refléter
/// le window_change SSH (régression du gel htop au redimensionnement).
#[tokio::test]
async fn openssh_resize_pty_stty_size() {
    if std::env::var("HARAKA_SSHD_TEST").ok().as_deref() != Some("1") {
        eprintln!("interop ignorée (HARAKA_SSHD_TEST != 1) — voir tests/sshd/setup.sh");
        return;
    }
    let key_path = std::env::var("HARAKA_TEST_KEY").expect("HARAKA_TEST_KEY requis");
    let user = std::env::var("HARAKA_TEST_USER").unwrap_or_else(|_| "harakats-test".into());
    let port: u16 = std::env::var("HARAKA_TEST_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(2222);
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

    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    manager
        .open(
            OpenRequest {
                session_id: "interop-resize".into(),
                hops: vec![],
                target: HopConfig {
                    host: "127.0.0.1".into(),
                    port,
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
    assert!(
        wait_connected(&manager, "interop-resize").await,
        "connexion openssh non établie"
    );

    manager
        .write("interop-resize", b"stty size\r".to_vec())
        .await
        .unwrap();
    assert!(
        read_until(&mut out_rx, "24 80", 10).await,
        "taille PTY initiale 24×80 non vue via stty size"
    );

    manager
        .resize("interop-resize", 100, 40)
        .await
        .expect("resize window_change");
    manager
        .write("interop-resize", b"stty size\r".to_vec())
        .await
        .unwrap();
    assert!(
        read_until(&mut out_rx, "40 100", 10).await,
        "le window_change SSH n'est pas appliqué au PTY serveur (stty size)"
    );

    manager.close("interop-resize").await.unwrap();
}
