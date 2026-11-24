//! Tunnels SSH (M7) : commandes IPC, état live (`tunnel://state`), démarrage
//! automatique à chaque (re)connexion. Les redirections elles-mêmes vivent dans
//! `haraka_ssh::tunnel` ; ce module ne manipule que des specs et des instantanés.
//!
//! Audit : `tunnel_start` / `tunnel_stop` avec le type et le port d'écoute
//! uniquement (jamais la destination ni un contenu).

use haraka_core::model::TunnelSpec;
use haraka_ssh::{TunnelDef, TunnelEvent, TunnelInfo, TunnelKind, TunnelState};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Pont événements du `TunnelManager` -> webview (`tunnel://state`).
pub fn spawn_forwarder(app: AppHandle) {
    let rx = app.state::<AppState>().tunnel_events.lock().unwrap().take();
    let Some(mut rx) = rx else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = app.emit("tunnel://state", &ev);
        }
    });
}

/// Tunnel ad hoc (non enregistré dans la connexion).
#[derive(Deserialize)]
pub struct AdHocSpec {
    pub kind: String,
    #[serde(default)]
    pub bind_addr: String,
    #[serde(default)]
    pub bind_port: u16,
    pub dest_host: Option<String>,
    pub dest_port: Option<u16>,
}

/// Spec enregistrée `tunnel_id` de la connexion de la session.
async fn saved_spec(state: &AppState, session: &str, tunnel_id: &str) -> CmdResult<TunnelSpec> {
    let conn_id = state
        .session_meta
        .lock()
        .await
        .get(session)
        .map(|m| m.connection_id.clone())
        .ok_or("session introuvable")?;
    let inner = state.inner.lock().await;
    let vault = inner.vault.as_ref().ok_or("coffre verrouillé")?;
    vault
        .store
        .connections
        .iter()
        .find(|c| c.id == conn_id)
        .and_then(|c| c.tunnels.iter().find(|t| t.id == tunnel_id))
        .cloned()
        .ok_or_else(|| "tunnel enregistré introuvable".to_string())
}

fn audit(state: &AppState, event: &str, info: &TunnelInfo) {
    let kind = match info.kind {
        TunnelKind::Local => "local",
        TunnelKind::Remote => "remote",
        TunnelKind::Dynamic => "dynamic",
    };
    let _ = haraka_core::audit::append(
        &state.data_dir,
        event,
        None,
        Some(&format!("kind={kind} port={}", info.bind_port)),
    );
}

async fn start_def(state: &AppState, session: &str, def: TunnelDef) -> CmdResult<TunnelInfo> {
    let info = state.tunnels.start(session, def).await.map_err(err)?;
    if info.state != TunnelState::Error {
        audit(state, "tunnel_start", &info);
    }
    Ok(info)
}

/// Démarre un tunnel de la session : soit une spec enregistrée de sa connexion
/// (`tunnelId`), soit une spec ad hoc (`spec`).
#[tauri::command]
pub async fn tunnel_start(
    state: State<'_, AppState>,
    session: String,
    tunnel_id: Option<String>,
    spec: Option<AdHocSpec>,
) -> CmdResult<TunnelInfo> {
    let def = match (tunnel_id, spec) {
        (Some(id), _) => {
            let saved = saved_spec(&state, &session, &id).await?;
            TunnelDef::from_spec(&saved).map_err(err)?
        }
        (None, Some(s)) => TunnelDef::from_spec(&TunnelSpec {
            id: format!("adhoc-{}", uuid::Uuid::new_v4().simple()),
            kind: s.kind,
            bind_addr: s.bind_addr,
            bind_port: s.bind_port,
            dest_host: s.dest_host,
            dest_port: s.dest_port,
            auto_start: false,
        })
        .map_err(err)?,
        (None, None) => return Err("tunnelId ou spec requis".into()),
    };
    start_def(&state, &session, def).await
}

#[tauri::command]
pub async fn tunnel_stop(
    state: State<'_, AppState>,
    session: String,
    tunnel_id: String,
) -> CmdResult<()> {
    let info = state
        .tunnels
        .list()
        .into_iter()
        .find(|i| i.session == session && i.id == tunnel_id);
    state
        .tunnels
        .stop(&session, &tunnel_id)
        .await
        .map_err(err)?;
    if let Some(info) = info.filter(|i| i.state != TunnelState::Error) {
        audit(&state, "tunnel_stop", &info);
    }
    Ok(())
}

/// Tunnels live de toutes les sessions.
#[tauri::command]
pub async fn tunnel_list(state: State<'_, AppState>) -> CmdResult<Vec<TunnelInfo>> {
    Ok(state.tunnels.list())
}

/// Arrête les tunnels d'une session (fermeture, déconnexion).
pub async fn stop_session(state: &AppState, session: &str) {
    state.tunnels.stop_all(session).await;
}

/// Arrête tous les tunnels (verrouillage du coffre).
pub async fn stop_everything(state: &AppState) {
    state.tunnels.stop_everything().await;
}

/// Démarre les tunnels « démarrage automatique » de la connexion de la session.
/// Appelé à chaque (re)connexion : un tunnel déjà actif n'est pas dupliqué.
/// Les erreurs sont journalisées et publiées en état `error`, jamais fatales.
pub async fn auto_start(app: &AppHandle, sid: &str) {
    let state = app.state::<AppState>();
    let Some(conn_id) = state
        .session_meta
        .lock()
        .await
        .get(sid)
        .map(|m| m.connection_id.clone())
    else {
        return;
    };
    let specs: Vec<TunnelSpec> = {
        let inner = state.inner.lock().await;
        let Some(vault) = inner.vault.as_ref() else {
            return;
        };
        vault
            .store
            .connections
            .iter()
            .find(|c| c.id == conn_id)
            .map(|c| c.tunnels.iter().filter(|t| t.auto_start).cloned().collect())
            .unwrap_or_default()
    };
    for spec in specs {
        let result = match TunnelDef::from_spec(&spec) {
            Ok(def) => start_def(&state, sid, def).await,
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = result {
            tracing::warn!(session = %sid, tunnel = %spec.id, "démarrage automatique du tunnel : {e}");
            let _ = app.emit(
                "tunnel://state",
                &TunnelEvent::Changed {
                    info: error_info(sid, &spec, e),
                },
            );
        }
    }
}

fn error_info(session: &str, spec: &TunnelSpec, error: String) -> TunnelInfo {
    TunnelInfo {
        id: spec.id.clone(),
        session: session.to_string(),
        kind: spec.kind.parse().unwrap_or(TunnelKind::Local),
        bind_addr: spec.bind_addr.clone(),
        bind_port: spec.bind_port,
        dest_host: spec.dest_host.clone().unwrap_or_default(),
        dest_port: spec.dest_port.unwrap_or(0),
        state: TunnelState::Error,
        error: Some(error),
        active: 0,
        bytes_out: 0,
        bytes_in: 0,
    }
}
