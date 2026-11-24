//! Interop multi-exec (M7) contre un vrai OpenSSH (conteneur de dev / CI) :
//! deux sessions réelles, la saisie de l'une est recopiée dans l'autre,
//! mêmes octets, même ordre. Activé par `HARAKA_SSHD_TEST=1`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use haraka_ssh::{
    AuthMaterial, HopConfig, HostKeyDecision, MultiExec, OpenRequest, ReconnectPolicy,
    SessionEvent, SessionManager, write_routed,
};
use secrecy::SecretString;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

fn enabled() -> bool {
    if std::env::var("HARAKA_SSHD_TEST").ok().as_deref() != Some("1") {
        eprintln!("interop multi-exec ignorée (HARAKA_SSHD_TEST != 1)");
        return false;
    }
    true
}

type Captured = Arc<Mutex<Vec<u8>>>;

struct Env {
    manager: Arc<SessionManager>,
    _tmp: tempfile::TempDir,
}

async fn start(env: &Env, sid: &str) -> Captured {
    let key_path = std::env::var("HARAKA_TEST_KEY").expect("HARAKA_TEST_KEY requis");
    let user = std::env::var("HARAKA_TEST_USER").unwrap_or_else(|_| "harakats-test".into());
    let port: u16 = std::env::var("HARAKA_TEST_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(2222);
    let pem = std::fs::read_to_string(&key_path).expect("clé de test lisible");
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let cap: Captured = Arc::new(Mutex::new(Vec::new()));
    let c = cap.clone();
    tokio::spawn(async move {
        while let Some(chunk) = out_rx.recv().await {
            c.lock().unwrap().extend_from_slice(&chunk);
        }
    });
    env.manager
        .open(
            OpenRequest {
                session_id: sid.into(),
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
                cols: 200,
                rows: 24,
                keepalive_s: 0,
                reconnect: ReconnectPolicy {
                    enabled: false,
                    max_attempts: 0,
                    backoff_max_s: 1,
                },
                known_hosts: env._tmp.path().join("known_hosts"),
            },
            out_tx,
        )
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while env.manager.states().await.get(sid).map(String::as_str) != Some("connected") {
        assert!(
            tokio::time::Instant::now() < deadline,
            "connexion non établie"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cap
}

fn env() -> Env {
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
    Env {
        manager,
        _tmp: tempfile::tempdir().unwrap(),
    }
}

/// Attend qu'une ligne de la sortie vérifie `pred` ; la retourne.
async fn wait_line(cap: &Captured, pred: impl Fn(&str) -> bool) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let text = String::from_utf8_lossy(&cap.lock().unwrap()).to_string();
        if let Some(l) = text
            .lines()
            .map(|l| l.rsplit('\r').next().unwrap_or(l).trim())
            .find(|l| pred(l))
        {
            return l.to_string();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "sortie attendue absente : {text:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn read_remote(env: &Env, sid: &str, path: &str) -> String {
    let s = env.manager.sftp(sid).await.unwrap();
    let mut f = s.open(path.to_string()).await.unwrap();
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).await.unwrap();
    String::from_utf8(buf).unwrap()
}

fn is_path(l: &str, tag: &str) -> bool {
    l.starts_with(&format!("/tmp/{tag}-"))
        && l[format!("/tmp/{tag}-").len()..]
            .chars()
            .all(|c| c.is_ascii_digit())
}

#[tokio::test]
async fn both_sessions_receive_same_bytes_in_order() {
    if !enabled() {
        return;
    }
    let env = env();
    let tag = format!("mx{}", std::process::id());
    let cap1 = start(&env, "m1").await;
    let cap2 = start(&env, "m2").await;
    let multi = Arc::new(Mutex::new(MultiExec::new()));
    let mgr = env.manager.clone();

    // Avant activation : la saisie de m1 ne va que dans m1.
    write_routed(
        &mgr,
        &multi,
        "m1",
        format!("echo solo-{tag}\r").into_bytes(),
    )
    .await
    .unwrap();
    wait_line(&cap1, |l| l == format!("solo-{tag}")).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !String::from_utf8_lossy(&cap2.lock().unwrap()).contains(&format!("solo-{tag}")),
        "pas de diffusion avant activation"
    );

    // Activation (≥ 2 membres requis).
    assert!(multi.lock().unwrap().set(true, &["m1".into()]).is_err());
    multi
        .lock()
        .unwrap()
        .set(true, &["m1".into(), "m2".into()])
        .unwrap();

    // Saisie en plusieurs morceaux, dont une ligne par session : fichier par shell (`$$`).
    let mut script = format!("f=/tmp/{tag}-$$\r");
    for i in 1..=200 {
        script.push_str(&format!("echo line{i} >> $f\r"));
    }
    script.push_str("echo $f\r");
    for chunk in script.as_bytes().chunks(37) {
        write_routed(&mgr, &multi, "m1", chunk.to_vec())
            .await
            .unwrap();
    }
    let p1 = wait_line(&cap1, |l| is_path(l, &tag)).await;
    let p2 = wait_line(&cap2, |l| is_path(l, &tag)).await;
    assert_ne!(p1, p2, "deux shells distincts");
    let expected: String = (1..=200).map(|i| format!("line{i}\n")).collect();
    assert_eq!(read_remote(&env, "m1", &p1).await, expected);
    assert_eq!(
        read_remote(&env, "m2", &p2).await,
        expected,
        "mêmes octets, même ordre"
    );

    // Depuis l'autre membre : symétrique, et aucune boucle (une seule copie par shell).
    write_routed(
        &mgr,
        &multi,
        "m2",
        format!("echo back >> {p1}; echo back >> {p2}\r").into_bytes(),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let r1 = read_remote(&env, "m1", &p1).await;
    // m2 → (m2 + m1) : chaque shell exécute les deux `echo` → 2 "back" par fichier, pas plus.
    assert_eq!(
        r1.matches("back\n").count(),
        2,
        "pas de rediffusion en boucle : {r1:?}"
    );

    // Un membre retiré ne reçoit plus rien.
    multi.lock().unwrap().remove("m2");
    assert!(!multi.lock().unwrap().is_enabled());
    write_routed(
        &mgr,
        &multi,
        "m1",
        format!("echo after >> {p1}\r").into_bytes(),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let r1 = read_remote(&env, "m1", &p1).await;
    assert_eq!(
        r1.matches("after\n").count(),
        1,
        "m2 ne l'a pas reçu : {r1:?}"
    );

    // Ménage.
    write_routed(
        &mgr,
        &multi,
        "m1",
        format!("rm -f {p1} {p2}\r").into_bytes(),
    )
    .await
    .unwrap();
    let _ = env.manager.close("m1").await;
    let _ = env.manager.close("m2").await;
}
