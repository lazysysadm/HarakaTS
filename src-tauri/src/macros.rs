//! Macros / snippets (M7) : envoi dans une session, snippet de démarrage,
//! menu contextuel du terminal natif.
//!
//! Le contenu est un SECRET chiffré dans le coffre : il n'est lu qu'ici, dans
//! le backend, au moment de l'envoi. Il ne sort vers l'interface que par
//! `macro_reveal` (audité) et `macro_preview` (aperçu pour la confirmation,
//! audité), jamais dans l'arbre.
//!
//! Choix documentés :
//! - Le contenu est envoyé comme des frappes (`haraka_core::macros::to_keystrokes` :
//!   chaque fin de ligne devient Entrée).
//! - `confirm_before_send` : l'envoi est refusé par le backend sans
//!   `confirmed: true`. L'interface (ou le dialogue GTK du menu) affiche
//!   d'abord l'aperçu exact (`paste_preview`).
//! - Snippet de démarrage : ne bloque jamais sur un dialogue. Une macro avec
//!   confirmation est IGNORÉE et une note est écrite dans le terminal. Il est
//!   envoyé une seule fois par session (pas à chaque reconnexion automatique).

use haraka_core::vault::SecretField;
use secrecy::ExposeSecret;
use tauri::{AppHandle, Manager, State};

use crate::state::{AppState, InputHandle, M7State, MacroEntry};

type CmdResult<T> = Result<T, String>;

const LOCKED: &str = "coffre verrouillé";
const STARTUP_DELAY_MS: u64 = 700;

struct Loaded {
    name: String,
    confirm: bool,
    content: String,
}

fn load(vault: &haraka_core::Vault, id: &str) -> CmdResult<Loaded> {
    let m = vault
        .store
        .macros
        .iter()
        .find(|m| m.id == id)
        .ok_or("macro introuvable")?;
    let content = vault
        .secret(id, SecretField::MacroContent)
        .map_err(|e| e.to_string())?;
    Ok(Loaded {
        name: m.name.clone(),
        confirm: m.confirm_before_send,
        content: content.expose_secret().to_string(),
    })
}

/// Met à jour le cache du menu contextuel (noms seulement).
pub fn refresh_menu(m7: &M7State, store: &haraka_core::Store) {
    if let Ok(mut menu) = m7.macro_menu.lock() {
        *menu = store
            .macros
            .iter()
            .map(|m| MacroEntry {
                id: m.id.clone(),
                name: m.name.clone(),
                confirm: m.confirm_before_send,
            })
            .collect();
    }
}

/// Entrées du menu « Macros » (appelé depuis le thread GTK : jamais bloquant,
/// repli sur le dernier cache si le coffre est occupé).
pub fn menu_entries(app: &AppHandle) -> Vec<(String, String, bool)> {
    let state = app.state::<AppState>();
    if let Ok(inner) = state.inner.try_lock()
        && let Some(v) = &inner.vault
    {
        refresh_menu(&state.m7, &v.store);
    }
    state
        .m7
        .macro_menu
        .lock()
        .map(|m| {
            m.iter()
                .map(|e| (e.id.clone(), e.name.clone(), e.confirm))
                .collect()
        })
        .unwrap_or_default()
}

async fn input_of(state: &AppState, session: &str) -> CmdResult<InputHandle> {
    let connected = state
        .sessions
        .states()
        .await
        .get(session)
        .map(String::as_str)
        == Some("connected");
    if !connected {
        return Err("session non connectée".into());
    }
    state
        .session_io
        .lock()
        .await
        .get(session)
        .map(|io| io.input.clone())
        .ok_or_else(|| "session introuvable".into())
}

fn audit_send(state: &AppState, session: &str, macro_id: &str, via: &str) {
    let _ = haraka_core::audit::append(
        &state.data_dir,
        "macro_send",
        Some(session),
        Some(&format!("macro={macro_id}, via={via}")),
    );
}

/// Action explicite « afficher » : le contenu sort une seule fois, journalisé.
#[tauri::command]
pub async fn macro_reveal(state: State<'_, AppState>, id: String) -> CmdResult<String> {
    let inner = state.inner.lock().await;
    let vault = inner.vault.as_ref().ok_or(LOCKED)?;
    let loaded = load(vault, &id)?;
    haraka_core::audit::append(vault.data_dir(), "macro_reveal", Some(&id), Some("content"))
        .map_err(|e| e.to_string())?;
    Ok(loaded.content)
}

#[derive(serde::Serialize)]
pub struct MacroPreview {
    /// Aperçu exact, caractères de contrôle rendus visibles.
    pub preview: String,
    pub lines: usize,
    pub bytes: usize,
}

/// Aperçu pour la confirmation (même rendu que la confirmation de collage). Audité.
#[tauri::command]
pub async fn macro_preview(state: State<'_, AppState>, id: String) -> CmdResult<MacroPreview> {
    let inner = state.inner.lock().await;
    let vault = inner.vault.as_ref().ok_or(LOCKED)?;
    let loaded = load(vault, &id)?;
    let _ =
        haraka_core::audit::append(vault.data_dir(), "macro_reveal", Some(&id), Some("preview"));
    Ok(MacroPreview {
        preview: haraka_core::paste::paste_preview(&loaded.content),
        lines: haraka_core::macros::split_lines(&loaded.content)
            .len()
            .max(1),
        bytes: loaded.content.len(),
    })
}

/// Envoie la macro dans la session. Une macro « confirmer avant envoi » est
/// refusée sans `confirmed: true` (l'interface a montré l'aperçu exact).
#[tauri::command]
pub async fn macro_send(
    state: State<'_, AppState>,
    macro_id: String,
    session: String,
    confirmed: Option<bool>,
) -> CmdResult<()> {
    let loaded = {
        let inner = state.inner.lock().await;
        let vault = inner.vault.as_ref().ok_or(LOCKED)?;
        load(vault, &macro_id)?
    };
    if loaded.confirm && confirmed != Some(true) {
        return Err("confirmation requise pour cette macro".into());
    }
    let input = input_of(&state, &session).await?;
    (input.broadcast)(haraka_core::macros::to_keystrokes(&loaded.content));
    audit_send(&state, &session, &macro_id, "ui");
    Ok(())
}

/// Macro choisie dans le menu contextuel du terminal : confirmation GTK avec
/// aperçu exact si la macro le demande, puis envoi.
pub async fn send_from_menu(app: &AppHandle, sid: &str, macro_id: &str) {
    let state = app.state::<AppState>();
    let loaded = {
        let inner = state.inner.lock().await;
        let Some(vault) = inner.vault.as_ref() else {
            return;
        };
        match load(vault, macro_id) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("macro (menu) : {e}");
                return;
            }
        }
    };
    let input = match input_of(&state, sid).await {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(session = %sid, "macro (menu) : {e}");
            return;
        }
    };
    let keys = haraka_core::macros::to_keystrokes(&loaded.content);
    let (app2, sid2, mid) = (app.clone(), sid.to_string(), macro_id.to_string());
    let send = move || {
        (input.broadcast)(keys);
        audit_send(&app2.state::<AppState>(), &sid2, &mid, "menu");
    };
    if !loaded.confirm {
        send();
        return;
    }
    #[cfg(target_os = "linux")]
    if let Some(win) = crate::vte::get(sid) {
        crate::vte::ask_macro_confirm(
            win,
            loaded.name.clone(),
            haraka_core::paste::paste_preview(&loaded.content),
            haraka_core::macros::split_lines(&loaded.content)
                .len()
                .max(1),
            loaded.content.len(),
            Box::new(move |ok| {
                if ok {
                    send();
                }
            }),
        );
    }
}

/// Envoie le snippet de démarrage de la connexion (si configuré) une fois
/// connecté — une seule fois par session, après ~700 ms (premier prompt).
pub async fn run_startup_snippet(app: &AppHandle, sid: &str) {
    let state = app.state::<AppState>();
    let conn_id = match state.session_meta.lock().await.get(sid) {
        Some(m) => m.connection_id.clone(),
        None => return,
    };
    let (macro_id, loaded) = {
        let inner = state.inner.lock().await;
        let Some(vault) = inner.vault.as_ref() else {
            return;
        };
        let Some(id) = vault
            .store
            .connections
            .iter()
            .find(|c| c.id == conn_id)
            .and_then(|c| c.startup_snippet_id.clone())
        else {
            return;
        };
        match load(vault, &id) {
            Ok(l) => (id, l),
            Err(e) => {
                tracing::warn!(session = %sid, "snippet de démarrage : {e}");
                return;
            }
        }
    };
    // Une seule fois par session : les reconnexions automatiques ne le rejouent pas.
    if !state
        .m7
        .startup_done
        .lock()
        .unwrap()
        .insert(sid.to_string())
    {
        return;
    }
    tokio::time::sleep(std::time::Duration::from_millis(STARTUP_DELAY_MS)).await;
    if loaded.confirm {
        // Ne bloque jamais sur un dialogue : ignoré, avec une note dans le terminal.
        #[cfg(target_os = "linux")]
        if let Some(win) = crate::vte::get(sid) {
            let note = crate::i18n::tr_with(
                &win.locale,
                "native.macro.startupSkipped",
                &[("name", &loaded.name)],
            );
            crate::vte::feed(&win, format!("\r\n\x1b[33m{note}\x1b[0m\r\n"));
        }
        let _ = haraka_core::audit::append(
            &state.data_dir,
            "macro_startup_skipped",
            Some(sid),
            Some(&format!("macro={macro_id}")),
        );
        return;
    }
    let input = match input_of(&state, sid).await {
        Ok(i) => i,
        Err(e) => {
            tracing::debug!(session = %sid, "snippet de démarrage non envoyé : {e}");
            state.m7.startup_done.lock().unwrap().remove(sid);
            return;
        }
    };
    (input.local)(haraka_core::macros::to_keystrokes(&loaded.content));
    audit_send(&state, sid, &macro_id, "startup");
}
