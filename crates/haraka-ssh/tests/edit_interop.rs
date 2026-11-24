//! Interop édition à distance (M6) contre un vrai OpenSSH (conteneur de dev / CI).
//! Activé par `HARAKA_SSHD_TEST=1` — voir `tests/sshd/setup.sh`.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use haraka_ssh::sftp;
use haraka_ssh::{
    AuthMaterial, EditEvent, EditManager, HopConfig, HostKeyDecision, OpenRequest, ReconnectPolicy,
    SaveOutcome, SessionEvent, SessionManager, build_editor_argv,
};
use secrecy::SecretString;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

fn enabled() -> bool {
    if std::env::var("HARAKA_SSHD_TEST").ok().as_deref() != Some("1") {
        eprintln!("interop édition ignorée (HARAKA_SSHD_TEST != 1) — voir tests/sshd/setup.sh");
        return false;
    }
    true
}

struct Env {
    manager: Arc<SessionManager>,
    edits: Arc<EditManager>,
    events: mpsc::UnboundedReceiver<EditEvent>,
    tmp_root: std::path::PathBuf,
    _tmp: tempfile::TempDir,
}

async fn connect(sid: &str) -> Env {
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
    let (out_tx, _out_rx) = mpsc::unbounded_channel();
    manager
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
    let tmp_root = tmp.path().join("tmp");
    std::fs::create_dir_all(&tmp_root).unwrap();
    let (edits, events) = EditManager::new(manager.clone(), tmp_root.clone());
    Env {
        manager,
        edits,
        events,
        tmp_root,
        _tmp: tmp,
    }
}

/// Dossier distant inscriptible (créé PAR le serveur, donc propriété de l'utilisateur de test).
async fn user_dir(env: &Env, sid: &str, name: &str) -> String {
    let s = env.manager.sftp(sid).await.unwrap();
    let path = format!("/home/harakats-test/m6-{name}-{}", std::process::id());
    if let Ok(true) = s.try_exists(path.as_str()).await {
        sftp::remove(&s, &path, true).await.unwrap();
    }
    sftp::mkdir(&s, &path).await.unwrap();
    path
}

async fn put(s: &russh_sftp::client::SftpSession, path: &str, data: &[u8]) {
    let mut f = s.create(path.to_string()).await.unwrap();
    f.write_all(data).await.unwrap();
    f.shutdown().await.unwrap();
}

async fn list_names(s: &russh_sftp::client::SftpSession, dir: &str) -> Vec<String> {
    sftp::list(s, dir)
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.name)
        .collect()
}

#[tokio::test]
async fn open_creates_private_temp_save_roundtrip_and_cleanup() {
    if !enabled() {
        return;
    }
    let env = connect("e1").await;
    let dir = user_dir(&env, "e1", "rt").await;
    let s = env.manager.sftp("e1").await.unwrap();
    let path = format!("{dir}/notes.txt");
    put(&s, &path, b"bonjour\nmonde\n").await;
    sftp::chmod(&s, &path, 0o640, false).await.unwrap();

    let (info, bytes) = env.edits.open("e1", &path).await.unwrap();
    assert_eq!(bytes, b"bonjour\nmonde\n");
    assert_eq!(info.name, "notes.txt");
    assert!(info.utf8);
    assert_eq!(info.mode, Some(0o640));

    // Fichiers temporaires privés.
    let ddir = env.tmp_root.join(&info.id);
    let dmode = std::fs::metadata(&ddir).unwrap().permissions().mode() & 0o777;
    let fmode = std::fs::metadata(ddir.join("notes.txt"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!((dmode, fmode), (0o700, 0o600));

    // Enregistrement : contenu, droits conservés, aucun résidu distant.
    match env
        .edits
        .save_bytes(&info.id, b"nouveau contenu\n".to_vec(), false)
        .await
        .unwrap()
    {
        SaveOutcome::Saved { info } => assert_eq!(info.size, 16),
        other => panic!("attendu Saved, obtenu {other:?}"),
    }
    assert_eq!(s.read(path.clone()).await.unwrap(), b"nouveau contenu\n");
    assert_eq!(sftp::stat(&s, &path).await.unwrap().mode, Some(0o640));
    assert_eq!(
        list_names(&s, &dir).await,
        vec!["notes.txt"],
        "pas de .tmp/.bak résiduel"
    );

    // Deuxième enregistrement sans rouvrir : la référence a été mise à jour (pas de faux conflit).
    assert!(matches!(
        env.edits
            .save_bytes(&info.id, b"v3\n".to_vec(), false)
            .await
            .unwrap(),
        SaveOutcome::Saved { .. }
    ));

    // Fermeture : plus rien en local.
    env.edits.close(&info.id).await.unwrap();
    assert!(!ddir.exists(), "répertoire temporaire supprimé");
    sftp::remove(&s, &dir, true).await.unwrap();
}

#[tokio::test]
async fn conflict_detection_and_three_resolutions() {
    if !enabled() {
        return;
    }
    let env = connect("e2").await;
    let dir = user_dir(&env, "e2", "cf").await;
    let s = env.manager.sftp("e2").await.unwrap();
    let path = format!("{dir}/conf.txt");
    put(&s, &path, b"original\n").await;
    let (info, _) = env.edits.open("e2", &path).await.unwrap();

    // 1. `touch` seul (mtime change, contenu identique) : PAS un conflit.
    let attrs = russh_sftp::protocol::FileAttributes {
        atime: Some(1_700_000_000),
        mtime: Some(1_700_000_000),
        ..russh_sftp::protocol::FileAttributes::empty()
    };
    s.set_metadata(path.clone(), attrs).await.unwrap();
    assert!(matches!(
        env.edits
            .save_bytes(&info.id, b"edit local 1\n".to_vec(), false)
            .await
            .unwrap(),
        SaveOutcome::Saved { .. }
    ));

    // 2. Le distant change réellement entre-temps → conflit, distant intact.
    put(&s, &path, b"modifie par quelqu'un d'autre\n").await;
    match env
        .edits
        .save_bytes(&info.id, b"edit local 2\n".to_vec(), false)
        .await
        .unwrap()
    {
        SaveOutcome::Conflict { remote } => {
            assert!(remote.exists);
            assert_eq!(remote.size, 30);
        }
        other => panic!("attendu Conflict, obtenu {other:?}"),
    }
    assert_eq!(
        s.read(path.clone()).await.unwrap(),
        "modifie par quelqu'un d'autre\n".as_bytes()
    );

    // Comparer : le contenu distant est lisible.
    let remote = env.edits.remote_bytes(&info.id).await.unwrap().unwrap();
    assert_eq!(remote, "modifie par quelqu'un d'autre\n".as_bytes());

    // 3a. Recharger : repart de l'état distant, plus de conflit.
    let (_, bytes) = env.edits.reload(&info.id).await.unwrap();
    assert_eq!(bytes, "modifie par quelqu'un d'autre\n".as_bytes());
    assert!(matches!(
        env.edits
            .save_bytes(&info.id, b"apres reload\n".to_vec(), false)
            .await
            .unwrap(),
        SaveOutcome::Saved { .. }
    ));

    // 3b. Écraser : même scénario, l'utilisateur impose sa version.
    put(&s, &path, b"encore une autre version distante\n").await;
    assert!(matches!(
        env.edits
            .save_bytes(&info.id, b"ma version\n".to_vec(), false)
            .await
            .unwrap(),
        SaveOutcome::Conflict { .. }
    ));
    assert!(matches!(
        env.edits
            .save_bytes(&info.id, b"ma version\n".to_vec(), true)
            .await
            .unwrap(),
        SaveOutcome::Saved { .. }
    ));
    assert_eq!(s.read(path.clone()).await.unwrap(), b"ma version\n");

    // 4. Fichier supprimé côté distant → conflit « n'existe plus » ; écraser le recrée.
    sftp::remove(&s, &path, false).await.unwrap();
    match env
        .edits
        .save_bytes(&info.id, b"recree\n".to_vec(), false)
        .await
        .unwrap()
    {
        SaveOutcome::Conflict { remote } => assert!(!remote.exists),
        other => panic!("attendu Conflict(exists=false), obtenu {other:?}"),
    }
    assert!(env.edits.remote_bytes(&info.id).await.unwrap().is_none());
    assert!(matches!(
        env.edits
            .save_bytes(&info.id, b"recree\n".to_vec(), true)
            .await
            .unwrap(),
        SaveOutcome::Saved { .. }
    ));
    assert_eq!(s.read(path.clone()).await.unwrap(), b"recree\n");
    assert_eq!(list_names(&s, &dir).await, vec!["conf.txt"]);
    env.edits.close_all().await;
    sftp::remove(&s, &dir, true).await.unwrap();
}

#[tokio::test]
async fn line_endings_and_bytes_preserved() {
    if !enabled() {
        return;
    }
    let env = connect("e3").await;
    let dir = user_dir(&env, "e3", "eol").await;
    let s = env.manager.sftp("e3").await.unwrap();

    // CRLF, sans retour final : octet pour octet.
    let crlf = b"ligne 1\r\nligne 2\r\nfin sans newline";
    put(&s, &format!("{dir}/win.txt"), crlf).await;
    let (info, bytes) = env
        .edits
        .open("e3", &format!("{dir}/win.txt"))
        .await
        .unwrap();
    assert_eq!(bytes, crlf);
    assert_eq!(serde_json::to_value(info.line_ending).unwrap(), "crlf");
    env.edits
        .save_bytes(&info.id, bytes.clone(), false)
        .await
        .unwrap();
    assert_eq!(s.read(format!("{dir}/win.txt")).await.unwrap(), crlf);

    // Non UTF-8 : signalé, octets intacts.
    let latin1 = b"caf\xe9\n";
    put(&s, &format!("{dir}/latin1.txt"), latin1).await;
    let (info, bytes) = env
        .edits
        .open("e3", &format!("{dir}/latin1.txt"))
        .await
        .unwrap();
    assert!(!info.utf8);
    env.edits.save_bytes(&info.id, bytes, false).await.unwrap();
    assert_eq!(s.read(format!("{dir}/latin1.txt")).await.unwrap(), latin1);

    // Un dossier n'est pas éditable.
    assert!(env.edits.open("e3", &dir).await.is_err());
    env.edits.close_all().await;
    sftp::remove(&s, &dir, true).await.unwrap();
}

#[tokio::test]
async fn oversized_file_is_refused() {
    if !enabled() {
        return;
    }
    let env = connect("e4").await;
    let dir = user_dir(&env, "e4", "big").await;
    let s = env.manager.sftp("e4").await.unwrap();
    let path = format!("{dir}/big.bin");
    put(&s, &path, &vec![b'x'; 21 * 1024 * 1024]).await;
    let err = env.edits.open("e4", &path).await.unwrap_err().to_string();
    assert!(err.contains("volumineux"), "{err}");
    assert!(
        std::fs::read_dir(&env.tmp_root).unwrap().next().is_none(),
        "aucun temporaire créé"
    );
    sftp::remove(&s, &dir, true).await.unwrap();
}

/// Un lecteur concurrent ne voit JAMAIS un fichier partiel : ancien contenu complet,
/// nouveau contenu complet, (ou, très brièvement, fichier absent pendant le remplacement).
#[tokio::test]
async fn upload_is_atomic_for_concurrent_readers() {
    if !enabled() {
        return;
    }
    let env = connect("e5").await;
    let reader = connect("e5r").await;
    let dir = user_dir(&env, "e5", "atom").await;
    let s = env.manager.sftp("e5").await.unwrap();
    let path = format!("{dir}/atom.txt");
    let old = vec![b'A'; 3 * 1024 * 1024];
    let new = vec![b'B'; 6 * 1024 * 1024];
    put(&s, &path, &old).await;
    let (info, _) = env.edits.open("e5", &path).await.unwrap();

    let rs = reader.manager.sftp("e5r").await.unwrap();
    let rpath = path.clone();
    let (old2, new2) = (old.clone(), new.clone());
    let reader_task = tokio::spawn(async move {
        let mut seen_new = false;
        let mut partial = 0usize;
        for _ in 0..200 {
            if let Ok(data) = rs.read(rpath.clone()).await {
                if data == new2 {
                    seen_new = true;
                } else if data != old2 {
                    partial += 1;
                }
            }
            if seen_new {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        (seen_new, partial)
    });
    env.edits
        .save_bytes(&info.id, new.clone(), false)
        .await
        .unwrap();
    let (seen_new, partial) = reader_task.await.unwrap();
    assert_eq!(partial, 0, "un lecteur a vu un contenu partiel");
    assert!(seen_new || s.read(path.clone()).await.unwrap() == new);
    assert_eq!(list_names(&s, &dir).await, vec!["atom.txt"]);
    env.edits.close_all().await;
    sftp::remove(&s, &dir, true).await.unwrap();
}

/// Éditeur externe factice : modifie le fichier temporaire → envoi automatique.
#[tokio::test]
async fn external_editor_auto_upload_and_conflict() {
    if !enabled() {
        return;
    }
    let mut env = connect("e6").await;
    let dir = user_dir(&env, "e6", "ext").await;
    let s = env.manager.sftp("e6").await.unwrap();
    let path = format!("{dir}/ext.txt");
    put(&s, &path, b"debut\n").await;
    let (info, _) = env.edits.open("e6", &path).await.unwrap();

    let scripts = tempfile::tempdir().unwrap();
    let editor = scripts.path().join("fake-editor.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\nsleep 0.3\nprintf 'ajout externe\\n' >> \"$1\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
    let local = env.tmp_root.join(&info.id).join("ext.txt");
    let argv = build_editor_argv(Some(editor.to_str().unwrap()), local.to_str().unwrap()).unwrap();
    env.edits
        .open_external(&info.id, argv.clone())
        .await
        .unwrap();

    // Envoi automatique après la sauvegarde de l'éditeur.
    let ev = tokio::time::timeout(Duration::from_secs(10), env.events.recv())
        .await
        .expect("événement d'envoi automatique")
        .unwrap();
    assert!(matches!(ev, EditEvent::Saved { .. }), "{ev:?}");
    assert_eq!(
        s.read(path.clone()).await.unwrap(),
        b"debut\najout externe\n"
    );
    // Notre propre écriture locale ne relance pas d'envoi (pas de boucle).
    assert!(
        tokio::time::timeout(Duration::from_millis(1500), env.events.recv())
            .await
            .is_err(),
        "envoi en boucle détecté"
    );

    // Le distant change, puis l'éditeur enregistre à nouveau → conflit signalé, rien d'écrasé.
    put(&s, &path, b"autre personne\n").await;
    env.edits.open_external(&info.id, argv).await.unwrap();
    let ev = tokio::time::timeout(Duration::from_secs(10), env.events.recv())
        .await
        .expect("événement de conflit")
        .unwrap();
    assert!(matches!(ev, EditEvent::Conflict { .. }), "{ev:?}");
    assert_eq!(s.read(path.clone()).await.unwrap(), b"autre personne\n");
    // Résolution « écraser » depuis le fichier temporaire.
    assert!(matches!(
        env.edits.save_local(&info.id, true).await.unwrap(),
        SaveOutcome::Saved { .. }
    ));
    let remote = s.read(path.clone()).await.unwrap();
    assert!(String::from_utf8_lossy(&remote).contains("ajout externe"));
    env.edits.close_all().await;
    sftp::remove(&s, &dir, true).await.unwrap();
}

/// Purge au démarrage : un crash ne laisse pas de temporaires d'édition.
#[test]
fn startup_purges_leftover_edit_temp_dirs() {
    let root = tempfile::tempdir().unwrap();
    let dd = haraka_core::DataDir::open(root.path().to_path_buf()).unwrap();
    let leftover = dd.tmp_dir().join("crashed-session");
    std::fs::create_dir_all(&leftover).unwrap();
    std::fs::write(leftover.join("secret.txt"), b"contenu").unwrap();
    let dd2 = haraka_core::DataDir::open(root.path().to_path_buf()).unwrap();
    assert!(!leftover.exists(), "temporaire résiduel purgé au démarrage");
    assert!(dd2.tmp_dir().exists());
}
