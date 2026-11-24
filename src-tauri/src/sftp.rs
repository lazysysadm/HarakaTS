//! Commandes SFTP et transferts (jalon M5).
//!
//! Le panneau SFTP utilise la MÊME connexion SSH que le terminal (second canal,
//! sous-système `sftp`) : aucune seconde authentification. Les opérations
//! distantes passent par `haraka_ssh::sftp` ; les transferts par le
//! `TransferManager` (file, reprise, parallélisme). La navigation locale est
//! limitée à du listing en lecture (`local_list`).

use std::sync::Arc;

use haraka_ssh::sftp::{self, Listing, RemoteEntry};
use haraka_ssh::{SshError, TransferEvent, TransferInfo, TransferManager, TransferSpec};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Session SFTP de la session SSH `session` (ouverte au premier appel).
async fn sftp_of(state: &AppState, session: &str) -> CmdResult<Arc<haraka_ssh::SftpSession>> {
    state.sessions.sftp(session).await.map_err(err)
}

/// Invalide la session SFTP si l'erreur indique un canal mort.
async fn finish<T>(state: &AppState, session: &str, result: Result<T, SshError>) -> CmdResult<T> {
    match result {
        Ok(v) => Ok(v),
        Err(e) => {
            if sftp::is_fatal(&e) {
                state.sessions.sftp_invalidate(session).await;
            }
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn sftp_home(state: State<'_, AppState>, session: String) -> CmdResult<String> {
    let s = sftp_of(&state, &session).await?;
    let r = sftp::canonicalize(&s, ".").await;
    finish(&state, &session, r).await
}

#[tauri::command]
pub async fn sftp_list(
    state: State<'_, AppState>,
    session: String,
    path: String,
) -> CmdResult<Listing> {
    let s = sftp_of(&state, &session).await?;
    let r = sftp::list(&s, &path).await;
    finish(&state, &session, r).await
}

#[tauri::command]
pub async fn sftp_stat(
    state: State<'_, AppState>,
    session: String,
    path: String,
) -> CmdResult<RemoteEntry> {
    let s = sftp_of(&state, &session).await?;
    let r = sftp::stat(&s, &path).await;
    finish(&state, &session, r).await
}

#[tauri::command]
pub async fn sftp_mkdir(
    state: State<'_, AppState>,
    session: String,
    path: String,
) -> CmdResult<()> {
    let s = sftp_of(&state, &session).await?;
    let r = sftp::mkdir(&s, &path).await;
    finish(&state, &session, r).await
}

#[tauri::command]
pub async fn sftp_rename(
    state: State<'_, AppState>,
    session: String,
    from: String,
    to: String,
) -> CmdResult<()> {
    let s = sftp_of(&state, &session).await?;
    let r = sftp::rename(&s, &from, &to).await;
    finish(&state, &session, r).await
}

#[tauri::command]
pub async fn sftp_remove(
    state: State<'_, AppState>,
    session: String,
    path: String,
    recursive: bool,
) -> CmdResult<()> {
    let s = sftp_of(&state, &session).await?;
    let r = sftp::remove(&s, &path, recursive).await;
    let _ = haraka_core::audit::append(
        &state.data_dir,
        "sftp_remove",
        Some(&session),
        Some(&format!("recursive={recursive}")),
    );
    finish(&state, &session, r).await
}

#[tauri::command]
pub async fn sftp_chmod(
    state: State<'_, AppState>,
    session: String,
    path: String,
    mode: u32,
    recursive: bool,
) -> CmdResult<()> {
    let s = sftp_of(&state, &session).await?;
    let r = sftp::chmod(&s, &path, mode, recursive).await;
    finish(&state, &session, r).await
}

#[tauri::command]
pub async fn sftp_symlink(
    state: State<'_, AppState>,
    session: String,
    link_path: String,
    target: String,
) -> CmdResult<()> {
    let s = sftp_of(&state, &session).await?;
    let r = sftp::symlink(&s, &link_path, &target).await;
    finish(&state, &session, r).await
}

// ------------------------------------------------------------------ local

#[derive(Serialize)]
pub struct LocalEntry {
    pub name: String,
    pub path: String,
    pub kind: sftp::EntryKind,
    pub size: u64,
    pub mtime: Option<u64>,
    pub mode: Option<u32>,
}

#[derive(Serialize)]
pub struct LocalListing {
    pub path: String,
    pub entries: Vec<LocalEntry>,
}

#[tauri::command]
pub async fn local_home() -> CmdResult<String> {
    std::env::var("HOME").map_err(|_| "HOME introuvable".into())
}

/// Listing local en lecture seule (panneau local du navigateur SFTP).
#[tauri::command]
pub async fn local_list(path: String) -> CmdResult<LocalListing> {
    use std::os::unix::fs::PermissionsExt;
    tauri::async_runtime::spawn_blocking(move || {
        let canon = std::fs::canonicalize(&path).map_err(|e| format!("{path} : {e}"))?;
        let mut entries = Vec::new();
        for item in std::fs::read_dir(&canon).map_err(|e| format!("{} : {e}", canon.display()))? {
            let Ok(item) = item else { continue };
            let Ok(lmeta) = item.metadata() else { continue };
            let is_link = lmeta.file_type().is_symlink();
            // Un lien vers un dossier se parcourt comme un dossier.
            let meta = if is_link {
                std::fs::metadata(item.path()).unwrap_or(lmeta)
            } else {
                lmeta
            };
            let kind = if meta.is_dir() {
                sftp::EntryKind::Dir
            } else if meta.is_file() {
                sftp::EntryKind::File
            } else {
                sftp::EntryKind::Other
            };
            entries.push(LocalEntry {
                name: item.file_name().to_string_lossy().into_owned(),
                path: item.path().to_string_lossy().into_owned(),
                kind,
                size: meta.len(),
                mtime: meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
                mode: Some(meta.permissions().mode() & 0o7777),
            });
        }
        Ok(LocalListing {
            path: canon.to_string_lossy().into_owned(),
            entries,
        })
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn local_mkdir(path: String) -> CmdResult<()> {
    tokio::fs::create_dir(&path).await.map_err(err)
}

/// Sélecteur de fichiers natif (envoi) : retourne les chemins choisis.
#[tauri::command]
pub async fn sftp_pick_files() -> CmdResult<Vec<String>> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Choisir les fichiers à envoyer")
            .pick_files()
    })
    .await
    .map_err(err)?;
    Ok(picked
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

/// Sélecteur de dossier natif (envoi d'un dossier, ou destination locale).
#[tauri::command]
pub async fn sftp_pick_folder() -> CmdResult<Option<String>> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Choisir un dossier")
            .pick_folder()
    })
    .await
    .map_err(err)?;
    Ok(picked.map(|p| p.to_string_lossy().into_owned()))
}

/// Snippet shell à copier (jamais injecté) : émet OSC 7 à chaque prompt.
#[tauri::command]
pub async fn sftp_shell_snippet() -> CmdResult<String> {
    Ok(SNIPPET.to_string())
}

const SNIPPET: &str = r#"# HarakaTS — suivi du répertoire courant (OSC 7). À coller dans ~/.bashrc ou ~/.zshrc.
__haraka_osc7() { printf '\033]7;file://%s%s\007' "${HOSTNAME:-localhost}" "$(printf %s "$PWD" | sed 's/ /%20/g')"; }
if [ -n "$ZSH_VERSION" ]; then precmd_functions+=(__haraka_osc7); else PROMPT_COMMAND="__haraka_osc7${PROMPT_COMMAND:+;$PROMPT_COMMAND}"; fi"#;

/// Ligne tapée dans le shell interactif pour activer le suivi du répertoire
/// (OSC 7), façon MobaXterm : bash (PROMPT_COMMAND) et zsh (precmd). Espace de
/// tête : exclue de l'historique si HISTCONTROL=ignorespace. Idempotente
/// (garde `__HARAKA_OSC7`). Aucun fichier distant n'est modifié : effet limité
/// à la session shell en cours.
pub const FOLLOW_HOOK: &str = r#" [ -z "$__HARAKA_OSC7" ] && { __HARAKA_OSC7=1; __haraka_osc7(){ local p=${PWD//\%/%25}; printf '\033]7;file://%s%s\007' "${HOSTNAME:-h}" "${p// /%20}"; }; if [ -n "$ZSH_VERSION" ]; then precmd_functions+=(__haraka_osc7); else PROMPT_COMMAND="__haraka_osc7${PROMPT_COMMAND:+;$PROMPT_COMMAND}"; fi; }; __haraka_osc7"#;

/// Active le suivi du répertoire sur la session : tape `FOLLOW_HOOK` + Entrée
/// dans le shell (action explicite de l'utilisateur ; voir SECURITY §7.1).
#[tauri::command]
pub async fn sftp_enable_follow(state: State<'_, AppState>, session: String) -> CmdResult<()> {
    inject_follow_hook(&state, &session).await?;
    let _ = haraka_core::audit::append(&state.data_dir, "sftp_follow_enable", Some(&session), None);
    Ok(())
}

pub async fn inject_follow_hook(state: &AppState, session: &str) -> CmdResult<()> {
    let mut line = FOLLOW_HOOK.as_bytes().to_vec();
    line.push(b'\r');
    #[cfg(target_os = "linux")]
    hide_echo(state, session).await;
    state.sessions.write(session, line).await.map_err(err)
}

/// Masque l'écho de la ligne d'activation dans la fenêtre VTE : attribut
/// « conceal » avant l'envoi, puis (côté sortie, dès le premier OSC 7) effacement
/// des lignes occupées et retour à l'affichage normal — le terminal ressemble à
/// un simple nouveau prompt. Garde-fou : après 4 s sans OSC 7 (shell non
/// supporté, ex. fish), l'attribut est levé pour ne pas laisser l'affichage masqué.
#[cfg(target_os = "linux")]
async fn hide_echo(state: &AppState, session: &str) {
    let Some(win) = crate::vte::get(session) else {
        return; // moteur xterm : repli, ligne visible
    };
    let ctx = state
        .session_io
        .lock()
        .await
        .get(session)
        .map(|io| io.clip.clone());
    let Some(ctx) = ctx else { return };
    let Ok(rows) = crate::vte::begin_conceal(&win, FOLLOW_HOOK.len()).await else {
        return;
    };
    ctx.hook_rows
        .store(rows, std::sync::atomic::Ordering::SeqCst);
    let pending = ctx.hook_rows.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        if pending.swap(0, std::sync::atomic::Ordering::SeqCst) != 0 {
            crate::vte::feed(&win, "\x1b[0m".into());
        }
    });
}

// ------------------------------------------------------------------ transferts

#[tauri::command]
pub async fn transfer_enqueue(
    state: State<'_, AppState>,
    specs: Vec<TransferSpec>,
) -> CmdResult<Vec<u64>> {
    // Parallélisme issu des réglages (coffre déverrouillé).
    let parallel = {
        let inner = state.inner.lock().await;
        inner
            .vault
            .as_ref()
            .map(|v| v.store.settings.transfer_parallelism as usize)
    };
    if let Some(n) = parallel {
        state.transfers.set_parallel(n);
    }
    let mut ids = Vec::new();
    for spec in specs {
        let _ = haraka_core::audit::append(
            &state.data_dir,
            "sftp_transfer",
            Some(&spec.session),
            Some(&format!("{:?}", spec.direction)),
        );
        ids.push(state.transfers.enqueue(spec));
    }
    Ok(ids)
}

#[tauri::command]
pub async fn transfer_pause(state: State<'_, AppState>, id: u64) -> CmdResult<()> {
    state.transfers.pause(id);
    Ok(())
}

#[tauri::command]
pub async fn transfer_resume(state: State<'_, AppState>, id: u64) -> CmdResult<()> {
    state.transfers.resume(id);
    Ok(())
}

#[tauri::command]
pub async fn transfer_cancel(state: State<'_, AppState>, id: u64) -> CmdResult<()> {
    state.transfers.cancel(id);
    Ok(())
}

#[tauri::command]
pub async fn transfer_list(state: State<'_, AppState>) -> CmdResult<Vec<TransferInfo>> {
    Ok(state.transfers.list())
}

#[tauri::command]
pub async fn transfer_clear(state: State<'_, AppState>) -> CmdResult<()> {
    state.transfers.clear_finished();
    Ok(())
}

/// Pont événements transferts → webview (`transfer://event`).
pub fn spawn_transfer_forwarder(mut rx: mpsc::UnboundedReceiver<TransferEvent>, app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        while let Some(TransferEvent::Update(info)) = rx.recv().await {
            let _ = app.emit("transfer://event", &info);
        }
    });
}

pub fn new_transfers(
    sessions: Arc<haraka_ssh::SessionManager>,
) -> (Arc<TransferManager>, mpsc::UnboundedReceiver<TransferEvent>) {
    TransferManager::new(sessions, 3)
}
