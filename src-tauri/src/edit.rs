//! Commandes d'édition à distance (jalon M6). Le moteur (téléchargement dans un
//! répertoire temporaire privé, détection de conflit, envoi atomique, éditeur
//! externe surveillé) est dans `haraka_ssh::edit` ; ici : IPC, réglages, audit.

use std::path::PathBuf;
use std::sync::Arc;

use haraka_ssh::{EditDocInfo, EditEvent, EditManager, SaveOutcome};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Document ouvert : info + contenu (UTF-8). `content` vaut `None` si le fichier
/// n'est pas de l'UTF-8 valide : seule l'édition externe est alors proposée.
#[derive(Serialize)]
pub struct EditOpenDto {
    info: EditDocInfo,
    content: Option<String>,
}

fn dto(info: EditDocInfo, bytes: Vec<u8>) -> EditOpenDto {
    let content = if info.utf8 {
        String::from_utf8(bytes).ok()
    } else {
        None
    };
    EditOpenDto { info, content }
}

#[tauri::command]
pub async fn edit_open(
    state: State<'_, AppState>,
    session: String,
    path: String,
) -> CmdResult<EditOpenDto> {
    let (info, bytes) = state.edits.open(&session, &path).await.map_err(err)?;
    // Audit : jamais le chemin ni le contenu.
    let _ = haraka_core::audit::append(&state.data_dir, "edit_open", Some(&session), None);
    Ok(dto(info, bytes))
}

#[tauri::command]
pub async fn edit_save(
    state: State<'_, AppState>,
    id: String,
    content: String,
    overwrite: bool,
) -> CmdResult<SaveOutcome> {
    let outcome = state
        .edits
        .save_bytes(&id, content.into_bytes(), overwrite)
        .await
        .map_err(err)?;
    audit_outcome(&state, &outcome, overwrite);
    Ok(outcome)
}

#[tauri::command]
pub async fn edit_save_local(
    state: State<'_, AppState>,
    id: String,
    overwrite: bool,
) -> CmdResult<SaveOutcome> {
    let outcome = state.edits.save_local(&id, overwrite).await.map_err(err)?;
    audit_outcome(&state, &outcome, overwrite);
    Ok(outcome)
}

fn audit_outcome(state: &AppState, outcome: &SaveOutcome, overwrite: bool) {
    let (action, detail) = match outcome {
        SaveOutcome::Saved { .. } if overwrite => ("edit_save", "overwrite"),
        SaveOutcome::Saved { .. } => ("edit_save", "ok"),
        SaveOutcome::Conflict { .. } => ("edit_conflict", "detected"),
    };
    let _ = haraka_core::audit::append(&state.data_dir, action, None, Some(detail));
}

#[tauri::command]
pub async fn edit_remote_content(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<Option<String>> {
    let bytes = state.edits.remote_bytes(&id).await.map_err(err)?;
    Ok(bytes.and_then(|b| String::from_utf8(b).ok()))
}

#[tauri::command]
pub async fn edit_local_content(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<Option<String>> {
    let bytes = state.edits.local_bytes(&id).await.map_err(err)?;
    Ok(String::from_utf8(bytes).ok())
}

#[tauri::command]
pub async fn edit_reload(state: State<'_, AppState>, id: String) -> CmdResult<EditOpenDto> {
    let (info, bytes) = state.edits.reload(&id).await.map_err(err)?;
    Ok(dto(info, bytes))
}

/// Lance l'éditeur externe configuré (Réglages → Éditeur externe ; vide = application
/// par défaut du système) sur le fichier temporaire. Le backend lance le programme
/// directement (argv, jamais via un shell ni un plugin shell Tauri).
#[tauri::command]
pub async fn edit_open_external(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let configured = {
        let inner = state.inner.lock().await;
        inner
            .vault
            .as_ref()
            .and_then(|v| v.store.settings.external_editor.clone())
    };
    let file: PathBuf = state.edits.local_path(&id).await.map_err(err)?;
    let argv = haraka_ssh::build_editor_argv(configured.as_deref(), &file.to_string_lossy())
        .map_err(err)?;
    state.edits.open_external(&id, argv).await.map_err(err)?;
    let _ = haraka_core::audit::append(&state.data_dir, "edit_external", None, None);
    Ok(())
}

#[tauri::command]
pub async fn edit_close(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.edits.close(&id).await.map_err(err)
}

#[tauri::command]
pub async fn edit_list(state: State<'_, AppState>) -> CmdResult<Vec<EditDocInfo>> {
    Ok(state.edits.list().await)
}

pub fn new_edits(
    sessions: Arc<haraka_ssh::SessionManager>,
    tmp_root: PathBuf,
) -> (Arc<EditManager>, mpsc::UnboundedReceiver<EditEvent>) {
    EditManager::new(sessions, tmp_root)
}

/// Pont événements édition → webview (`edit://event`).
pub fn spawn_edit_forwarder(mut rx: mpsc::UnboundedReceiver<EditEvent>, app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = app.emit("edit://event", &ev);
        }
    });
}
