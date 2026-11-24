//! Interop SFTP (M5) contre un vrai OpenSSH (conteneur de dev / CI).
//! Activé par `HARAKA_SSHD_TEST=1` — voir `tests/sshd/setup.sh`.
//!
//! Le serveur tourne dans le même conteneur : le « distant » est le système de
//! fichiers local, accédé en tant que `harakats-test` (permissions réelles).

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use haraka_ssh::sftp;
use haraka_ssh::{
    AuthMaterial, Conflict, Direction, HopConfig, HostKeyDecision, OpenRequest, ReconnectPolicy,
    SessionEvent, SessionManager, SymlinkPolicy, TransferManager, TransferSpec, TransferState,
};
use secrecy::SecretString;
use tokio::sync::mpsc;

fn enabled() -> bool {
    if std::env::var("HARAKA_SSHD_TEST").ok().as_deref() != Some("1") {
        eprintln!("interop SFTP ignorée (HARAKA_SSHD_TEST != 1) — voir tests/sshd/setup.sh");
        return false;
    }
    true
}

struct Env {
    manager: Arc<SessionManager>,
    out_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    _tmp: tempfile::TempDir,
}

/// Ouvre une session SSH réelle (clé ed25519) sous l'identifiant `sid`.
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
    let (out_tx, out_rx) = mpsc::unbounded_channel();
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
    wait_connected(&manager, sid).await;
    Env {
        manager,
        out_rx,
        _tmp: tmp,
    }
}

async fn wait_connected(manager: &SessionManager, sid: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if manager.states().await.get(sid).map(String::as_str) == Some("connected") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("connexion openssh non établie");
}

/// Dossier « distant » de travail (créé par root, lisible par le serveur).
fn remote_dir(name: &str) -> PathBuf {
    let p = PathBuf::from("/home/harakats-test").join(format!("m5-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// Dossier distant créé PAR le serveur (appartient à harakats-test, inscriptible).
async fn user_dir(env: &Env, sid: &str, name: &str) -> String {
    let s = env.manager.sftp(sid).await.unwrap();
    let path = format!("/home/harakats-test/m5u-{name}-{}", std::process::id());
    if let Ok(true) = s.try_exists(path.as_str()).await {
        sftp::remove(&s, &path, true).await.unwrap();
    }
    sftp::mkdir(&s, &path).await.unwrap();
    path
}

/// Crée (ou remplace) un fichier distant : `SftpSession::write` n'a pas CREATE.
async fn put(s: &russh_sftp::client::SftpSession, path: String, data: &[u8]) {
    use tokio::io::AsyncWriteExt;
    let mut f = s.create(path).await.unwrap();
    f.write_all(data).await.unwrap();
    f.shutdown().await.unwrap();
}

fn write_random(path: &Path, size: usize) {
    use rand::Rng;
    let mut f = std::fs::File::create(path).unwrap();
    let mut rng = rand::rng();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut left = size;
    while left > 0 {
        let n = left.min(buf.len());
        rng.fill_bytes(&mut buf[..n]);
        f.write_all(&buf[..n]).unwrap();
        left -= n;
    }
}

fn same_content(a: &Path, b: &Path) -> bool {
    let (mut fa, mut fb) = (
        std::fs::File::open(a).unwrap(),
        std::fs::File::open(b).unwrap(),
    );
    if fa.metadata().unwrap().len() != fb.metadata().unwrap().len() {
        return false;
    }
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let n = fa.read(&mut ba).unwrap();
        if n == 0 {
            return true;
        }
        fb.read_exact(&mut bb[..n]).unwrap();
        if ba[..n] != bb[..n] {
            return false;
        }
    }
}

fn spec(sid: &str, dir: Direction, source: &str, dest_dir: &str) -> TransferSpec {
    TransferSpec {
        session: sid.into(),
        direction: dir,
        source: source.into(),
        dest_dir: dest_dir.into(),
        conflict: Conflict::Overwrite,
        symlinks: SymlinkPolicy::Skip,
        preserve: true,
    }
}

async fn wait_state(tm: &TransferManager, id: u64, want: TransferState, secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if tm.list().iter().any(|i| i.id == id && i.state == want) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

fn info(tm: &TransferManager, id: u64) -> haraka_ssh::TransferInfo {
    tm.list().into_iter().find(|i| i.id == id).unwrap()
}

#[tokio::test]
async fn crud_listing_chmod_symlink() {
    if !enabled() {
        return;
    }
    let env = connect("crud").await;
    let dir = user_dir(&env, "crud", "crud").await;
    let s = env.manager.sftp("crud").await.unwrap();

    sftp::mkdir(&s, &format!("{dir}/sub")).await.unwrap();
    put(&s, format!("{dir}/a.txt"), b"hello").await;
    sftp::rename(&s, &format!("{dir}/a.txt"), &format!("{dir}/b.txt"))
        .await
        .unwrap();
    sftp::symlink(&s, &format!("{dir}/ln"), &format!("{dir}/sub"))
        .await
        .unwrap();
    sftp::chmod(&s, &format!("{dir}/b.txt"), 0o600, false)
        .await
        .unwrap();

    let listing = sftp::list(&s, &dir).await.unwrap();
    assert_eq!(listing.path, dir);
    let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"sub") && names.contains(&"b.txt") && names.contains(&"ln"));
    assert!(!names.contains(&"a.txt"), "renommé");
    let b = listing.entries.iter().find(|e| e.name == "b.txt").unwrap();
    assert_eq!(b.size, 5);
    assert_eq!(b.mode, Some(0o600));
    let ln = listing.entries.iter().find(|e| e.name == "ln").unwrap();
    assert_eq!(ln.kind, sftp::EntryKind::Symlink);
    assert!(ln.link_to_dir, "lien vers un dossier résolu");
    assert_eq!(
        ln.link_target.as_deref(),
        Some(format!("{dir}/sub").as_str())
    );

    // Suppression : dossier non vide refusé sans récursif, accepté avec.
    put(&s, format!("{dir}/sub/x"), b"x").await;
    assert!(
        sftp::remove(&s, &format!("{dir}/sub"), false)
            .await
            .is_err()
    );
    sftp::remove(&s, &format!("{dir}/sub"), true).await.unwrap();
    sftp::remove(&s, &dir, true).await.unwrap();
    assert!(!s.try_exists(dir.as_str()).await.unwrap());
}

/// Les octets SFTP ne doivent JAMAIS atteindre l'affichage du terminal.
#[tokio::test]
async fn sftp_bytes_never_reach_terminal() {
    if !enabled() {
        return;
    }
    let mut env = connect("iso").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    while env.out_rx.try_recv().is_ok() {} // bannière / prompt
    let dir = user_dir(&env, "iso", "iso").await;
    let s = env.manager.sftp("iso").await.unwrap();
    put(&s, format!("{dir}/f"), &vec![7u8; 200_000]).await;
    let _ = sftp::list(&s, &dir).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut got = Vec::new();
    while let Ok(chunk) = env.out_rx.try_recv() {
        got.extend(chunk);
    }
    assert!(
        !got.contains(&0u8) && !got.windows(3).any(|w| w == [7, 7, 7]),
        "données SFTP fuitées dans le terminal ({} octets)",
        got.len()
    );
    sftp::remove(&s, &dir, true).await.unwrap();
}

#[tokio::test]
async fn upload_download_roundtrip_preserves_mode_and_mtime() {
    if !enabled() {
        return;
    }
    let env = connect("rt").await;
    let (tm, _rx) = TransferManager::new(env.manager.clone(), 3);
    let local = tempfile::tempdir().unwrap();
    let src = local.path().join("data.bin");
    write_random(&src, 3 * 1024 * 1024 + 123);
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o640)).unwrap();
    let mtime = std::time::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    std::fs::File::options()
        .write(true)
        .open(&src)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    let rdir = user_dir(&env, "rt", "rt").await;

    let up = tm.enqueue(spec("rt", Direction::Upload, src.to_str().unwrap(), &rdir));
    assert!(
        wait_state(&tm, up, TransferState::Done, 30).await,
        "{:?}",
        info(&tm, up)
    );
    let s = env.manager.sftp("rt").await.unwrap();
    let st = sftp::stat(&s, &format!("{rdir}/data.bin")).await.unwrap();
    assert_eq!(st.size, 3 * 1024 * 1024 + 123);
    assert_eq!(st.mode, Some(0o640));
    assert_eq!(st.mtime, Some(1_600_000_000));
    assert!(!s.try_exists(format!("{rdir}/data.bin.part")).await.unwrap());

    let back = tempfile::tempdir().unwrap();
    let down = tm.enqueue(spec(
        "rt",
        Direction::Download,
        &format!("{rdir}/data.bin"),
        back.path().to_str().unwrap(),
    ));
    assert!(
        wait_state(&tm, down, TransferState::Done, 30).await,
        "{:?}",
        info(&tm, down)
    );
    let out = back.path().join("data.bin");
    assert!(same_content(&src, &out));
    let m = std::fs::metadata(&out).unwrap();
    assert_eq!(m.permissions().mode() & 0o7777, 0o640);
    assert_eq!(
        m.modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        1_600_000_000
    );
    sftp::remove(&s, &rdir, true).await.unwrap();
}

/// Reprise d'un gros fichier (100 Mio+) après pause, puis après coupure de la
/// connexion : octets identiques à la fin.
#[tokio::test]
async fn large_download_pause_resume_and_connection_drop() {
    if !enabled() {
        return;
    }
    let env = connect("big").await;
    let (tm, _rx) = TransferManager::new(env.manager.clone(), 3);
    let rdir = remote_dir("big");
    let remote_file = rdir.join("big.bin");
    write_random(&remote_file, 110 * 1024 * 1024);
    std::fs::set_permissions(&remote_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let back = tempfile::tempdir().unwrap();
    let dest_dir = back.path().to_str().unwrap().to_string();
    let part = back.path().join("big.bin.part");
    let dest = back.path().join("big.bin");

    // 1) pause en cours de route
    let id = tm.enqueue(spec(
        "big",
        Direction::Download,
        remote_file.to_str().unwrap(),
        &dest_dir,
    ));
    loop {
        let i = info(&tm, id);
        assert!(
            i.state != TransferState::Done,
            "terminé trop vite pour tester la pause"
        );
        if i.done > 10 * 1024 * 1024 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    tm.pause(id);
    assert!(wait_state(&tm, id, TransferState::Paused, 10).await);
    let partial = std::fs::metadata(&part).unwrap().len();
    assert!(
        partial > 0 && partial < 110 * 1024 * 1024,
        "part conservé : {partial}"
    );
    assert!(!dest.exists(), "pas de fichier final tant que non terminé");

    // 2) reprise : repart de l'offset du .part
    tm.resume(id);
    loop {
        let i = info(&tm, id);
        if i.state == TransferState::Done {
            panic!("terminé avant la coupure simulée");
        }
        if i.done > partial + 10 * 1024 * 1024 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    // 3) coupure de la connexion SSH en plein transfert
    env.manager.close("big").await.unwrap();
    assert!(
        wait_state(&tm, id, TransferState::Error, 20).await,
        "{:?}",
        info(&tm, id)
    );
    let cut = std::fs::metadata(&part).unwrap().len();
    assert!(cut > partial, "le .part a progressé avant la coupure");

    // 4) nouvelle session, même transfert : reprise depuis le .part
    let env2 = connect("big2").await;
    let (tm2, _rx2) = TransferManager::new(env2.manager.clone(), 3);
    let id2 = tm2.enqueue(spec(
        "big2",
        Direction::Download,
        remote_file.to_str().unwrap(),
        &dest_dir,
    ));
    assert!(
        wait_state(&tm2, id2, TransferState::Done, 120).await,
        "{:?}",
        info(&tm2, id2)
    );
    assert!(
        same_content(&remote_file, &dest),
        "fichier identique après reprises"
    );
    assert!(!part.exists());
    let _ = std::fs::remove_dir_all(&rdir);
}

#[tokio::test]
async fn large_upload_resume_after_pause() {
    if !enabled() {
        return;
    }
    let env = connect("upl").await;
    let (tm, _rx) = TransferManager::new(env.manager.clone(), 3);
    let local = tempfile::tempdir().unwrap();
    let src = local.path().join("up.bin");
    write_random(&src, 105 * 1024 * 1024);
    let rdir = user_dir(&env, "upl", "upl").await;
    let id = tm.enqueue(spec("upl", Direction::Upload, src.to_str().unwrap(), &rdir));
    loop {
        let i = info(&tm, id);
        assert!(i.state != TransferState::Done, "terminé trop vite");
        if i.done > 10 * 1024 * 1024 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    tm.pause(id);
    assert!(wait_state(&tm, id, TransferState::Paused, 10).await);
    let s = env.manager.sftp("upl").await.unwrap();
    let partial = s
        .metadata(format!("{rdir}/up.bin.part"))
        .await
        .unwrap()
        .len();
    assert!(partial > 0 && partial < 105 * 1024 * 1024);
    tm.resume(id);
    assert!(
        wait_state(&tm, id, TransferState::Done, 120).await,
        "{:?}",
        info(&tm, id)
    );
    let back = tempfile::tempdir().unwrap();
    let id2 = tm.enqueue(spec(
        "upl",
        Direction::Download,
        &format!("{rdir}/up.bin"),
        back.path().to_str().unwrap(),
    ));
    assert!(wait_state(&tm, id2, TransferState::Done, 120).await);
    assert!(same_content(&src, &back.path().join("up.bin")));
    sftp::remove(&s, &rdir, true).await.unwrap();
}

#[tokio::test]
async fn corrupted_part_restarts_from_zero() {
    if !enabled() {
        return;
    }
    let env = connect("cor").await;
    let (tm, _rx) = TransferManager::new(env.manager.clone(), 3);
    let rdir = remote_dir("cor");
    let remote_file = rdir.join("f.bin");
    write_random(&remote_file, 2 * 1024 * 1024);
    std::fs::set_permissions(&remote_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let back = tempfile::tempdir().unwrap();
    // .part bidon (contenu différent de la source) de 1 Mio
    std::fs::write(back.path().join("f.bin.part"), vec![0xAAu8; 1024 * 1024]).unwrap();
    let id = tm.enqueue(spec(
        "cor",
        Direction::Download,
        remote_file.to_str().unwrap(),
        back.path().to_str().unwrap(),
    ));
    assert!(
        wait_state(&tm, id, TransferState::Done, 30).await,
        "{:?}",
        info(&tm, id)
    );
    assert!(
        same_content(&remote_file, &back.path().join("f.bin")),
        "reprise corrompue détectée"
    );
    let _ = std::fs::remove_dir_all(&rdir);
}

#[tokio::test]
async fn recursive_tree_with_perms_dates_and_symlink_policy() {
    if !enabled() {
        return;
    }
    let env = connect("rec").await;
    let (tm, _rx) = TransferManager::new(env.manager.clone(), 3);
    let local = tempfile::tempdir().unwrap();
    let root = local.path().join("tree");
    std::fs::create_dir_all(root.join("a/b")).unwrap();
    std::fs::write(root.join("top.txt"), b"top").unwrap();
    std::fs::write(root.join("a/mid.txt"), b"mid").unwrap();
    std::fs::write(root.join("a/b/deep.sh"), b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(
        root.join("a/b/deep.sh"),
        std::fs::Permissions::from_mode(0o750),
    )
    .unwrap();
    std::os::unix::fs::symlink("top.txt", root.join("link-to-top")).unwrap();
    let rdir = user_dir(&env, "rec", "rec").await;

    let id = tm.enqueue(spec(
        "rec",
        Direction::Upload,
        root.to_str().unwrap(),
        &rdir,
    ));
    assert!(
        wait_state(&tm, id, TransferState::Done, 30).await,
        "{:?}",
        info(&tm, id)
    );
    let i = info(&tm, id);
    assert_eq!(i.files_total, 3, "le lien est ignoré par défaut");
    assert_eq!(i.files_done, 3);
    let s = env.manager.sftp("rec").await.unwrap();
    assert!(
        s.try_exists(format!("{rdir}/tree/a/b/deep.sh"))
            .await
            .unwrap()
    );
    assert!(
        !s.try_exists(format!("{rdir}/tree/link-to-top"))
            .await
            .unwrap()
    );
    let deep = sftp::stat(&s, &format!("{rdir}/tree/a/b/deep.sh"))
        .await
        .unwrap();
    assert_eq!(deep.mode, Some(0o750));

    // Retour récursif
    let back = tempfile::tempdir().unwrap();
    let id2 = tm.enqueue(spec(
        "rec",
        Direction::Download,
        &format!("{rdir}/tree"),
        back.path().to_str().unwrap(),
    ));
    assert!(
        wait_state(&tm, id2, TransferState::Done, 30).await,
        "{:?}",
        info(&tm, id2)
    );
    assert_eq!(
        std::fs::read(back.path().join("tree/a/mid.txt")).unwrap(),
        b"mid"
    );
    assert_eq!(
        std::fs::metadata(back.path().join("tree/a/b/deep.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o750
    );
    sftp::remove(&s, &rdir, true).await.unwrap();
}

#[tokio::test]
async fn parallel_jobs_all_complete() {
    if !enabled() {
        return;
    }
    let env = connect("par").await;
    let (tm, _rx) = TransferManager::new(env.manager.clone(), 3);
    let local = tempfile::tempdir().unwrap();
    let rdir = user_dir(&env, "par", "par").await;
    let mut ids = Vec::new();
    for n in 0..6 {
        let f = local.path().join(format!("f{n}.bin"));
        write_random(&f, 4 * 1024 * 1024);
        ids.push(tm.enqueue(spec("par", Direction::Upload, f.to_str().unwrap(), &rdir)));
    }
    // Jamais plus de 3 jobs « running » simultanément.
    let mut max_running = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while tokio::time::Instant::now() < deadline {
        let list = tm.list();
        max_running = max_running.max(
            list.iter()
                .filter(|i| i.state == TransferState::Running)
                .count(),
        );
        if list.iter().all(|i| i.state == TransferState::Done) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        tm.list().iter().all(|i| i.state == TransferState::Done),
        "{:?}",
        tm.list()
    );
    assert!(max_running <= 3, "parallélisme borné : {max_running}");
    assert_eq!(tm.list().len(), ids.len());
    let s = env.manager.sftp("par").await.unwrap();
    sftp::remove(&s, &rdir, true).await.unwrap();
}

#[tokio::test]
async fn conflict_skip_keeps_existing_overwrite_replaces() {
    if !enabled() {
        return;
    }
    let env = connect("cfl").await;
    let (tm, _rx) = TransferManager::new(env.manager.clone(), 3);
    let local = tempfile::tempdir().unwrap();
    let f = local.path().join("c.txt");
    std::fs::write(&f, b"new-content").unwrap();
    let rdir = user_dir(&env, "cfl", "cfl").await;
    let s = env.manager.sftp("cfl").await.unwrap();
    put(&s, format!("{rdir}/c.txt"), b"old").await;

    let mut sp = spec("cfl", Direction::Upload, f.to_str().unwrap(), &rdir);
    sp.conflict = Conflict::Skip;
    let id = tm.enqueue(sp);
    assert!(wait_state(&tm, id, TransferState::Done, 20).await);
    assert_eq!(s.read(format!("{rdir}/c.txt")).await.unwrap(), b"old");

    let id = tm.enqueue(spec("cfl", Direction::Upload, f.to_str().unwrap(), &rdir));
    assert!(wait_state(&tm, id, TransferState::Done, 20).await);
    assert_eq!(
        s.read(format!("{rdir}/c.txt")).await.unwrap(),
        b"new-content"
    );
    sftp::remove(&s, &rdir, true).await.unwrap();
}

#[tokio::test]
async fn errors_permission_denied_and_invalid_path() {
    if !enabled() {
        return;
    }
    let env = connect("err").await;
    let (tm, _rx) = TransferManager::new(env.manager.clone(), 3);
    let local = tempfile::tempdir().unwrap();
    let f = local.path().join("x.txt");
    std::fs::write(&f, b"x").unwrap();

    // Écriture dans un dossier du système interdit à l'utilisateur.
    let id = tm.enqueue(spec("err", Direction::Upload, f.to_str().unwrap(), "/etc"));
    assert!(
        wait_state(&tm, id, TransferState::Error, 20).await,
        "{:?}",
        info(&tm, id)
    );
    let msg = info(&tm, id).error.unwrap().to_lowercase();
    assert!(
        msg.contains("permission") || msg.contains("denied") || msg.contains("failure"),
        "{msg}"
    );

    // Source distante inexistante.
    let id = tm.enqueue(spec(
        "err",
        Direction::Download,
        "/nonexistent/zzz",
        local.path().to_str().unwrap(),
    ));
    assert!(wait_state(&tm, id, TransferState::Error, 20).await);

    // Source locale inexistante.
    let id = tm.enqueue(spec("err", Direction::Upload, "/nonexistent/zzz", "/tmp"));
    assert!(wait_state(&tm, id, TransferState::Error, 20).await);

    // Opérations CRUD : chemins invalides → erreurs applicatives, session saine.
    let s = env.manager.sftp("err").await.unwrap();
    assert!(sftp::list(&s, "/nonexistent/zzz").await.is_err());
    assert!(sftp::mkdir(&s, "/proc/zzz").await.is_err());
    assert!(
        sftp::list(&s, "/").await.is_ok(),
        "la session reste utilisable"
    );

    // Annulation d'un job en erreur : état final cohérent + nettoyage de la liste.
    tm.clear_finished();
    assert!(tm.list().is_empty());
}
