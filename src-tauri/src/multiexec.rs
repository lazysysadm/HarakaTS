//! Multi-exec (M7) : couche IPC. La logique (adhésions, cibles, écriture
//! diffusée) vit dans `haraka_ssh::multiexec` ; ici : commandes, événement
//! `multiexec://state`, indicateur permanent dans la barre d'état des fenêtres
//! membres et audit.

use haraka_ssh::MultiExecState;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

const EVENT: &str = "multiexec://state";

fn snapshot(state: &AppState) -> MultiExecState {
    state.m7.multi_exec.lock().unwrap().snapshot()
}

/// Émet l'état et met à jour l'indicateur des fenêtres concernées (anciens et
/// nouveaux membres).
fn publish(app: &AppHandle, old_members: Vec<String>) {
    let state = app.state::<AppState>();
    let snap = snapshot(&state);
    let _ = app.emit(EVENT, &snap);
    #[cfg(target_os = "linux")]
    {
        let mut all = old_members;
        for s in &snap.sessions {
            if !all.contains(s) {
                all.push(s.clone());
            }
        }
        for sid in all {
            if let Some(win) = crate::vte::get(&sid) {
                let on = snap.enabled && snap.sessions.contains(&sid);
                crate::vte::set_multiexec(&win, on.then_some(snap.sessions.len()));
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = old_members;
}

/// Une session se ferme : elle quitte le multi-exec et son snippet de démarrage est oublié.
pub fn forget_session(app: &AppHandle, sid: &str) {
    let state = app.state::<AppState>();
    if let Ok(mut done) = state.m7.startup_done.lock() {
        done.remove(sid);
    }
    let (old, changed) = {
        let mut m = state.m7.multi_exec.lock().unwrap();
        let old = m.members();
        (old, m.remove(sid))
    };
    if changed {
        publish(app, old);
    }
}

/// Verrouillage du coffre : tout est coupé.
pub fn reset(app: &AppHandle) {
    let state = app.state::<AppState>();
    let old = {
        let mut m = state.m7.multi_exec.lock().unwrap();
        let old = m.members();
        let _ = m.set(false, &[]);
        old
    };
    publish(app, old);
}

#[tauri::command]
pub async fn multi_exec_state(state: State<'_, AppState>) -> CmdResult<MultiExecState> {
    Ok(snapshot(&state))
}

/// Active (≥ 2 sessions existantes) ou désactive le multi-exec.
#[tauri::command]
pub async fn multi_exec_set(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
    sessions: Vec<String>,
) -> CmdResult<MultiExecState> {
    if enabled {
        let known = state.session_meta.lock().await;
        if sessions.iter().any(|s| !known.contains_key(s)) {
            return Err("session introuvable".into());
        }
    }
    let old = {
        let mut m = state.m7.multi_exec.lock().unwrap();
        let old = m.members();
        m.set(enabled, &sessions).map_err(|e| e.to_string())?;
        old
    };
    let n = sessions.len().to_string();
    let _ = haraka_core::audit::append(
        &state.data_dir,
        if enabled {
            "multiexec_enable"
        } else {
            "multiexec_disable"
        },
        None,
        enabled.then(|| format!("sessions={n}")).as_deref(),
    );
    publish(&app, old);
    Ok(snapshot(&state))
}

/// Envoie une ligne saisie (texte + Entrée) à toutes les sessions membres :
/// elle passe par le chemin de saisie du premier membre, qui la recopie dans les autres.
#[tauri::command]
pub async fn multi_exec_send(state: State<'_, AppState>, text: String) -> CmdResult<usize> {
    let members = {
        let m = state.m7.multi_exec.lock().unwrap();
        if !m.is_enabled() {
            return Err("multi-exec inactif".into());
        }
        m.members()
    };
    let mut keys = haraka_core::macros::to_keystrokes(&text);
    if !keys.ends_with('\r') {
        keys.push('\r');
    }
    let input = {
        let io = state.session_io.lock().await;
        members
            .iter()
            .find_map(|s| io.get(s).map(|i| i.input.broadcast.clone()))
            .ok_or("session introuvable")?
    };
    input(keys);
    let _ = haraka_core::audit::append(
        &state.data_dir,
        "multiexec_send",
        None,
        Some(&format!("sessions={}, bytes={}", members.len(), text.len())),
    );
    Ok(members.len())
}
