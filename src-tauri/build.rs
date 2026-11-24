fn main() {
    // Moteur terminal natif VTE (M3-VTE) : libvte-2.91 (LGPL), API C stable.
    // Le probe pkg-config émet les métadonnées de lien (rustc-link-lib/search).
    #[cfg(target_os = "linux")]
    pkg_config::Config::new()
        .atleast_version("0.50")
        .probe("vte-2.91")
        .expect("libvte-2.91-dev requis (moteur terminal natif VTE, façon Remmina)");
    emit_git_hash();
    tauri_build::build()
}

/// Hash git court (+ `-dirty` si l'arbre a des modifications) exposé en `HARAKA_GIT_HASH`
/// pour l'écran « À propos » et `--version` ; `unknown` hors dépôt git.
fn emit_git_hash() {
    use std::process::Command;
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .args(["-c", "safe.directory=*"])
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let hash = git(&["rev-parse", "--short=7", "HEAD"]).filter(|h| !h.is_empty());
    let value = match hash {
        Some(h) => {
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
                .is_some_and(|s| !s.is_empty());
            if dirty { format!("{h}-dirty") } else { h }
        }
        None => "unknown".to_string(),
    };
    println!("cargo:rustc-env=HARAKA_GIT_HASH={value}");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");
}
