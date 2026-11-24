//! Couche IPC des sessions SSH (jalons M3 / M3-VTE).
//!
//! Les matériaux d'authentification sont résolus ICI (backend) depuis le
//! coffre déverrouillé — jamais depuis l'UI. La sortie terminal est décodée
//! selon l'encodage de la connexion puis envoyée vers une fenêtre native
//! GTK+VTE (seul moteur terminal) ; les événements (prompts, host keys,
//! état) deviennent des dialogues GTK et la barre d'état de cette fenêtre.

use std::sync::Arc;

use encoding_rs::Encoding;
use haraka_core::Vault;
use haraka_core::effective;
use haraka_core::knownhosts;
use haraka_core::vault::SecretField;
use haraka_ssh::{
    AuthMaterial, HopConfig, HostKeyDecision, OpenRequest, ReconnectPolicy, SessionManager,
};
use secrecy::ExposeSecret;
use serde::Serialize;
use tauri::{AppHandle, State};
use tokio::sync::mpsc;

use crate::state::{AppState, ClipCtx, SessionIo, SessionMeta};

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Construit les matériaux d'auth dans l'ordre effectif (agent, publickey,
/// password, keyboard-interactive), depuis le credential résolu.
fn materials_for(
    vault: &Vault,
    cred_id: Option<&str>,
    order: &[String],
) -> CmdResult<Vec<AuthMaterial>> {
    let cred = cred_id.and_then(|id| vault.store.credential(id).ok());
    let mut mats = Vec::new();
    for method in order {
        match method.as_str() {
            "agent" => mats.push(AuthMaterial::Agent),
            "publickey" => {
                if let Some(c) = cred
                    && c.private_key.is_some()
                {
                    let pem = vault.secret(&c.id, SecretField::PrivateKey).map_err(err)?;
                    let passphrase = if c.passphrase.is_some() {
                        Some(vault.secret(&c.id, SecretField::Passphrase).map_err(err)?)
                    } else {
                        None
                    };
                    mats.push(AuthMaterial::Key { pem, passphrase });
                }
            }
            "password" => {
                if let Some(c) = cred
                    && c.password.is_some()
                {
                    mats.push(AuthMaterial::Password(
                        vault.secret(&c.id, SecretField::Password).map_err(err)?,
                    ));
                }
            }
            "keyboard-interactive" => {
                let prefill = cred.and_then(|c| {
                    if c.password.is_some() {
                        vault.secret(&c.id, SecretField::Password).ok()
                    } else {
                        None
                    }
                });
                mats.push(AuthMaterial::KeyboardInteractive { prefill });
            }
            _ => {}
        }
    }
    Ok(mats)
}

/// Destination de la sortie terminal décodée : fenêtre native GTK+VTE
/// (seul moteur terminal, comme Remmina).
enum OutputSink {
    #[cfg(target_os = "linux")]
    Vte {
        ptr: usize,
        alive: Arc<std::sync::atomic::AtomicBool>,
        /// Tampon coalescé : un seul invoke GTK en vol par session.
        buf: Arc<std::sync::Mutex<String>>,
        in_flight: Arc<std::sync::atomic::AtomicBool>,
    },
}

/// Commandes d'une session VTE, traitées DANS L'ORDRE par une seule tâche
/// (saisie et resize ne peuvent plus se croiser ni s'inverser).
#[cfg(target_os = "linux")]
enum SessionCmd {
    /// Frappes clavier (signal « commit » de VTE) : jamais confirmées.
    Input(String),
    /// Collage explicite (Ctrl+Shift+V, clic milieu, menu) : déjà confirmé
    /// côté GTK si suspect ; encadré par le bracketed paste si le terminal l'a activé.
    Paste(String),
    /// Frappe locale uniquement (jamais diffusée par multi-exec) : snippet de démarrage.
    InputLocal(String),
    Resize(u32, u32),
}

/// Tâche de sortie : décode les octets serveur vers l'encodage terminal
/// (UTF-8 par défaut), **intercepte OSC 52** (M4 : politique + taille, les
/// séquences n'atteignent jamais le terminal) et pousse le texte vers le sink.
/// **Par lots** (16 ms / 32 Ko) : htop/tmux émettent des centaines de petits
/// paquets/seconde ; un message IPC par paquet sature la webview (gel).
fn spawn_output_forwarder(
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
    sink: OutputSink,
    encoding: &'static Encoding,
    clip: ClipCtx,
    mut log: Option<haraka_core::sessionlog::SessionLog>,
    highlight: bool,
) {
    tauri::async_runtime::spawn(async move {
        let mut highlighter = haraka_core::highlight::Highlighter::new(highlight);
        // Sans BOM sniffing : un flux terminal n'a pas de BOM, et le sniffing
        // peut retenir les premiers octets sans rien émettre.
        let mut decoder = encoding.new_decoder_without_bom_handling();
        let mut extractor = haraka_core::osc52::Osc52Extractor::new(clip.osc52_max);
        let mut osc7 = haraka_ssh::osc7::Osc7Scanner::new();
        let mut buf = String::new();
        loop {
            let received = if buf.is_empty() {
                if log.as_ref().is_some_and(|l| l.is_dirty()) {
                    // Journal en attente de vidage : ne pas attendre indéfiniment.
                    match tokio::time::timeout(haraka_core::sessionlog::FLUSH_INTERVAL, rx.recv())
                        .await
                    {
                        Ok(v) => v,
                        Err(_) => {
                            log_flush(&mut log);
                            continue;
                        }
                    }
                } else {
                    rx.recv().await
                }
            } else {
                match tokio::time::timeout(std::time::Duration::from_millis(16), rx.recv()).await {
                    Ok(v) => v,
                    Err(_) => {
                        flush_output(&mut buf, &sink);
                        continue;
                    }
                }
            };
            match received {
                Some(chunk) => {
                    let mut pending: &[u8] = &chunk;
                    while !pending.is_empty() {
                        // decode_to_string n'agrandit PAS le String : sa capacité
                        // est la limite de sortie (bug du terminal vide, 2026-11-20).
                        let cap = decoder
                            .max_utf8_buffer_length(pending.len())
                            .unwrap_or_else(|| pending.len().saturating_mul(3).saturating_add(8));
                        let mut text = String::with_capacity(cap.max(8));
                        let (result, read, _had_errors) =
                            decoder.decode_to_string(pending, &mut text, false);
                        pending = &pending[read..];
                        // Suivi du bracketed paste (DECSET 2004) : dernier marqueur gagnant.
                        match (text.rfind("\x1b[?2004h"), text.rfind("\x1b[?2004l")) {
                            (Some(a), Some(b)) => clip
                                .bracketed
                                .store(a > b, std::sync::atomic::Ordering::SeqCst),
                            (Some(_), None) => clip
                                .bracketed
                                .store(true, std::sync::atomic::Ordering::SeqCst),
                            (None, Some(_)) => clip
                                .bracketed
                                .store(false, std::sync::atomic::Ordering::SeqCst),
                            (None, None) => {}
                        }
                        // OSC 7 : le panneau SFTP peut suivre le répertoire du terminal.
                        if let Some(path) = osc7.push(&text)
                            && let Some(app) = &clip.app
                        {
                            use tauri::Emitter;
                            let _ = app.emit(
                                "sftp://cwd",
                                serde_json::json!({ "session": clip.session_id, "path": path }),
                            );
                        }
                        crate::post_connect::remember_tail(&clip.tail, &text);
                        // Activation masquée du suivi : dès le PREMIER OSC 7, on efface
                        // l'écho caché (N lignes au-dessus + ligne du prompt) et on lève
                        // le masquage, juste avant que le nouveau prompt ne s'affiche.
                        let hidden_rows = osc7
                            .first_end()
                            .map(|end| {
                                (
                                    end,
                                    clip.hook_rows.swap(0, std::sync::atomic::Ordering::SeqCst),
                                )
                            })
                            .filter(|(_, rows)| *rows > 0);
                        let (head, tail) = match hidden_rows {
                            Some((end, _)) => (&text[..end], Some(&text[end..])),
                            None => (&text[..], None),
                        };
                        for (i, part) in std::iter::once(head).chain(tail).enumerate() {
                            if i == 1
                                && let Some((_, rows)) = hidden_rows
                            {
                                buf.push_str(&format!("\x1b[0m\x1b[{rows}A\r\x1b[J"));
                            }
                            let (clean, ops) = extractor.push(part);
                            // Journal APRÈS l'extraction OSC 52 : jamais de charge presse-papier.
                            log_output(&mut log, &clean);
                            buf.push_str(&highlighter.apply(&clean));
                            for op in ops {
                                handle_osc52(&clip, op);
                            }
                        }
                        if result == encoding_rs::CoderResult::InputEmpty || read == 0 {
                            break;
                        }
                    }
                    if buf.len() >= 32_768 {
                        flush_output(&mut buf, &sink);
                    }
                }
                None => {
                    let tail = extractor.finish();
                    log_output(&mut log, &tail);
                    buf.push_str(&tail);
                    flush_output(&mut buf, &sink);
                    if let Some(l) = log.take() {
                        let bytes = l.finish().unwrap_or(0);
                        let _ = haraka_core::audit::append(
                            &clip.data_dir,
                            "session_log_stop",
                            Some(&clip.session_id),
                            Some(&format!("bytes={bytes}")),
                        );
                    }
                    break;
                }
            }
        }
        tracing::debug!("canal de sortie terminal fermé");
    });
}

/// Écrit dans le journal de session ; en cas d'erreur disque, la journalisation
/// s'arrête (la session continue).
fn log_output(log: &mut Option<haraka_core::sessionlog::SessionLog>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(l) = log
        && let Err(e) = l.write_output(text)
    {
        tracing::warn!("journal de session : écriture impossible, arrêt : {e}");
        *log = None;
    }
}

fn log_flush(log: &mut Option<haraka_core::sessionlog::SessionLog>) {
    if let Some(l) = log
        && let Err(e) = l.flush()
    {
        tracing::warn!("journal de session : vidage impossible, arrêt : {e}");
        *log = None;
    }
}

/// Applique la politique presse-papier effective aux opérations OSC 52.
/// Write (r2l) : écrit dans le presse-papier local si autorisé + taille OK.
/// Query (l2r) : renvoie le presse-papier local au distant si autorisé —
/// vecteur d'exfiltration, toujours audité. Le contenu n'est jamais journalisé.
fn handle_osc52(clip: &ClipCtx, op: haraka_core::osc52::Osc52Op) {
    use haraka_core::osc52::Osc52Op;
    match op {
        Osc52Op::Write { data, .. } => {
            if !clip.policy.r2l {
                tracing::debug!(session = %clip.session_id, "OSC 52 write refusé (politique)");
                return;
            }
            if data.len() > clip.osc52_max {
                tracing::warn!(
                    session = %clip.session_id,
                    bytes = data.len(),
                    "OSC 52 write refusé (taille > limite)"
                );
                return;
            }
            let Ok(text) = String::from_utf8(data) else {
                return;
            };
            let bytes = text.len();
            tauri::async_runtime::spawn_blocking(move || {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let _ = cb.set_text(text);
                }
            });
            let _ = haraka_core::audit::append(
                &clip.data_dir,
                "osc52_write",
                Some(&clip.session_id),
                Some(&format!("bytes={bytes}")),
            );
        }
        Osc52Op::Query { clipboard } => {
            if !clip.policy.l2r {
                tracing::debug!(session = %clip.session_id, "OSC 52 query refusée (politique)");
                return;
            }
            let clip = clip.clone();
            tauri::async_runtime::spawn(async move {
                let text = tauri::async_runtime::spawn_blocking(|| {
                    arboard::Clipboard::new()
                        .ok()
                        .and_then(|mut cb| cb.get_text().ok())
                        .unwrap_or_default()
                })
                .await
                .unwrap_or_default();
                if text.is_empty() || text.len() > clip.osc52_max {
                    let _ = haraka_core::audit::append(
                        &clip.data_dir,
                        "osc52_query",
                        Some(&clip.session_id),
                        Some(&format!("bytes={} refusée", text.len())),
                    );
                    return;
                }
                let _ = haraka_core::audit::append(
                    &clip.data_dir,
                    "osc52_query",
                    Some(&clip.session_id),
                    Some(&format!("bytes={}", text.len())),
                );
                let resp = haraka_core::osc52::query_response(&clipboard, text.as_bytes());
                let _ = clip
                    .sessions
                    .write(&clip.session_id, resp.into_bytes())
                    .await;
            });
        }
    }
}

fn flush_output(buf: &mut String, sink: &OutputSink) {
    if buf.is_empty() {
        return;
    }
    let text = std::mem::take(buf);
    tracing::debug!(chars = text.len(), "sortie terminal (lot)");
    match sink {
        #[cfg(target_os = "linux")]
        OutputSink::Vte {
            ptr,
            alive,
            buf,
            in_flight,
        } => {
            {
                let mut b = buf.lock().unwrap();
                b.push_str(&text);
                // Borne dure : si le thread GTK ne rend plus depuis longtemps,
                // on tronque plutôt que d'exploser la mémoire (htop se
                // redessine entièrement au rafraîchissement suivant).
                if b.len() > 16 * 1024 * 1024 {
                    tracing::warn!("tampon terminal saturé (>16 Mo) — flux tronqué");
                    b.clear();
                    b.push_str("\r\n\x1b[31m[flux tronqué : retard d'affichage]\x1b[0m\r\n");
                }
            }
            crate::vte::schedule_feed(*ptr, alive, buf, in_flight);
        }
    }
}

/// Plan d'ouverture résolu par le backend : requête SSH, encodage, contexte
/// presse-papier (M4) et protections de collage, partagés par les deux moteurs.
struct OpenPlan {
    req: OpenRequest,
    encoding: &'static Encoding,
    title: String,
    clip: ClipCtx,
    paste_confirm: bool,
    copy_on_select: bool,
    /// Nom de la connexion si la journalisation de session est activée.
    log_name: Option<String>,
}

/// Résolution backend (config effective + matériaux d'auth) partagée par
/// les deux moteurs : xterm web (`session_open`) et VTE natif.
async fn build_open_request(
    state: &AppState,
    connection_id: &str,
    cols: u32,
    rows: u32,
) -> CmdResult<OpenPlan> {
    let inner = state.inner.lock().await;
    let vault = inner.vault.as_ref().ok_or("coffre verrouillé")?;
    let conn = vault
        .store
        .connections
        .iter()
        .find(|c| c.id == connection_id)
        .ok_or("connexion introuvable")?
        .clone();
    let eff = effective::resolve(&vault.store, &conn);
    let order = eff.auth_method_order.value.clone().unwrap_or_default();
    let cred_username = eff
        .credential_id
        .value
        .as_deref()
        .and_then(|id| vault.store.credential(id).ok())
        .map(|c| c.username.clone());
    let user = eff
        .user
        .value
        .clone()
        .or(cred_username)
        .ok_or("utilisateur requis (connexion, dossier ou credential)")?;
    let target = HopConfig {
        host: eff.host.clone(),
        port: eff.port.value.unwrap_or(22),
        user: user.clone(),
        materials: materials_for(vault, eff.credential_id.value.as_deref(), &order)?,
    };
    let mut hops = Vec::new();
    for hop in eff.jump_chain.value.clone().unwrap_or_default() {
        hops.push(HopConfig {
            host: hop.host.clone(),
            port: hop.port.unwrap_or(22),
            user: hop.user.clone().unwrap_or_else(|| user.clone()),
            materials: materials_for(
                vault,
                hop.credential_id
                    .as_deref()
                    .or(eff.credential_id.value.as_deref()),
                &order,
            )?,
        });
    }
    let encoding = Encoding::for_label(
        eff.encoding
            .value
            .clone()
            .unwrap_or_else(|| "utf-8".into())
            .as_bytes(),
    )
    .unwrap_or(encoding_rs::UTF_8);
    let reconnect = ReconnectPolicy {
        enabled: vault.store.settings.reconnect.enabled,
        max_attempts: vault.store.settings.reconnect.max_attempts,
        backoff_max_s: vault.store.settings.reconnect.backoff_max_s,
    };
    let session_id = uuid::Uuid::new_v4().to_string();
    let req = OpenRequest {
        session_id: session_id.clone(),
        hops,
        target,
        cols,
        rows,
        keepalive_s: eff.keepalive_s.value.unwrap_or(30),
        reconnect,
        known_hosts: state.data_dir.known_hosts_file(),
    };
    let settings = &vault.store.settings;
    let clip = ClipCtx {
        session_id,
        sessions: state.sessions.clone(),
        data_dir: state.data_dir.clone(),
        policy: haraka_core::osc52::parse_policy(
            eff.clipboard_policy
                .value
                .as_deref()
                .unwrap_or("bidirectional"),
        ),
        osc52_max: settings.osc52_max_bytes,
        bracketed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        hook_rows: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        tail: Arc::new(std::sync::Mutex::new(String::new())),
        app: state.app_handle.get().cloned(),
    };
    crate::macros::refresh_menu(&state.m7, &vault.store);
    let log_name = conn.session_logging.then(|| conn.name.clone());
    Ok(OpenPlan {
        log_name,
        req,
        encoding,
        title: conn.name,
        clip,
        paste_confirm: settings.paste_confirm_multiline,
        copy_on_select: settings.copy_on_select,
    })
}

/// Adresse de la fenêtre GTK principale (terminaux en onglets). Le wrapper gtk
/// est relâché tout de suite : seule l'adresse (valide tant que l'app vit) sort.
#[cfg(target_os = "linux")]
fn main_gtk_ptr(app: &tauri::AppHandle) -> Option<usize> {
    use gtk::glib::object::ObjectType;
    use tauri::Manager;
    let win = app.get_webview_window("main")?.gtk_window().ok()?;
    Some(win.as_ptr() as usize)
}

/// Ouvre une session dans une **fenêtre native GTK+VTE** (moteur de Remmina,
/// défaut depuis M3-VTE). Le PTY n'est ouvert qu'aux dimensions réelles de la
/// fenêtre allouée ; clavier/resize/copier-coller sont gérés nativement.
#[tauri::command]
pub async fn session_open_vte(
    state: State<'_, AppState>,
    connection_id: String,
) -> CmdResult<String> {
    #[cfg(target_os = "linux")]
    {
        let mut plan = build_open_request(&state, &connection_id, 80, 24).await?;
        let (font_size, scrollback, locale, in_tabs, highlight) = {
            let inner = state.inner.lock().await;
            let settings = &inner
                .vault
                .as_ref()
                .ok_or("coffre verrouillé")?
                .store
                .settings;
            (
                settings.terminal_font_size,
                settings.terminal_scrollback,
                settings.locale.clone(),
                settings.terminal_in_tabs,
                settings.terminal_highlight,
            )
        };
        let main_window_ptr = if in_tabs {
            let ptr = state.app_handle.get().and_then(main_gtk_ptr);
            if ptr.is_none() {
                tracing::warn!(
                    "mode onglets demandé mais fenêtre principale GTK introuvable : fenêtre séparée"
                );
            }
            ptr
        } else {
            None
        };
        tracing::info!(
            in_tabs,
            tabs_active = main_window_ptr.is_some(),
            "mode d'affichage du terminal"
        );
        let session_id = plan.req.session_id.clone();
        let sessions = state.sessions.clone();
        let io = state.session_io.clone();
        let meta = state.session_meta.clone();

        // Une seule tâche consommatrice par session (spawn après open_window :
        // la confirmation de collage a besoin de la fenêtre). Saisie et resize
        // sont traités dans l'ordre ; l'encodeur vit dans la tâche.
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SessionCmd>();

        let cb = crate::vte::VteCallbacks {
            on_input: {
                let tx = cmd_tx.clone();
                Arc::new(move |text: String| {
                    let _ = tx.send(SessionCmd::Input(text));
                })
            },
            on_paste: {
                let tx = cmd_tx.clone();
                Arc::new(move |text: String| {
                    let _ = tx.send(SessionCmd::Paste(text));
                })
            },
            on_resize: {
                let tx = cmd_tx.clone();
                Arc::new(move |cols: u32, rows: u32| {
                    let _ = tx.send(SessionCmd::Resize(cols, rows));
                })
            },
            macro_entries: {
                let app = state.app_handle.get().cloned();
                Arc::new(move || {
                    app.as_ref()
                        .map(crate::macros::menu_entries)
                        .unwrap_or_default()
                })
            },
            on_macro: {
                let app = state.app_handle.get().cloned();
                let sid = session_id.clone();
                Arc::new(move |macro_id: String| {
                    if let Some(app) = app.clone() {
                        let sid = sid.clone();
                        tauri::async_runtime::spawn(async move {
                            crate::macros::send_from_menu(&app, &sid, &macro_id).await;
                        });
                    }
                })
            },
            on_close: {
                let app = state.app_handle.get().cloned();
                let sessions = sessions.clone();
                let io = io.clone();
                let meta = meta.clone();
                let sid = session_id.clone();
                Arc::new(move || {
                    if let Some(app) = &app {
                        crate::multiexec::forget_session(app, &sid);
                    }
                    let sessions = sessions.clone();
                    let io = io.clone();
                    let meta = meta.clone();
                    let sid = sid.clone();
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        tracing::info!(session = %sid, "fermeture de la session (fenêtre VTE fermée)");
                        match sessions.close(&sid).await {
                            Ok(()) => {}
                            // Session déjà retirée (échecs de reconnexion épuisés) : normal.
                            Err(haraka_ssh::SshError::NotFound(_)) => {
                                tracing::debug!(session = %sid, "session déjà fermée");
                            }
                            Err(e) => tracing::warn!(session = %sid, "close session : {e}"),
                        }
                        io.lock().await.remove(&sid);
                        meta.lock().await.remove(&sid);
                        if let Some(app) = &app {
                            use tauri::Emitter;
                            let _ =
                                app.emit("session://closed", serde_json::json!({ "session": sid }));
                        }
                    });
                })
            },
            on_focus: {
                let app = state.app_handle.get().cloned();
                let sid = session_id.clone();
                Arc::new(move || {
                    use tauri::Emitter;
                    if let Some(app) = &app {
                        let _ = app.emit("session://active", serde_json::json!({ "session": sid }));
                    }
                })
            },
            on_sftp: {
                let app = state.app_handle.get().cloned();
                let sid = session_id.clone();
                Arc::new(move || {
                    use tauri::{Emitter, Manager};
                    if let Some(app) = &app {
                        let _ = app.emit("sftp://open", serde_json::json!({ "session": sid }));
                        if let Some(main) = app.get_webview_window("main") {
                            let _ = main.show();
                            let _ = main.set_focus();
                        }
                    }
                })
            },
            on_reconnect: {
                let sessions = sessions.clone();
                let sid = session_id.clone();
                Arc::new(move || {
                    let sessions = sessions.clone();
                    let sid = sid.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = sessions.reconnect(&sid).await {
                            tracing::warn!(session = %sid, "reconnexion : {e}");
                        }
                    });
                })
            },
        };

        let opened = crate::vte::open_window(
            session_id.clone(),
            plan.title.clone(),
            cb,
            crate::vte::VteOptions {
                font_size,
                scrollback,
                copy_on_select: plan.copy_on_select,
                paste_confirm: plan.paste_confirm,
                locale,
                main_window_ptr,
            },
        )
        .await;
        let (win, cols, rows) = opened.map_err(err)?;
        if !win.alive.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("fenêtre VTE fermée avant l'ouverture de la session".into());
        }
        crate::vte::register(session_id.clone(), win.clone());
        plan.req.cols = cols;
        plan.req.rows = rows;

        // Consommateur ordonné : saisie, collage et resize passent dans l'ordre
        // d'arrivée. La confirmation de collage se fait côté GTK (vte.rs), AVANT
        // d'envoyer `Paste` : jamais sur les frappes (flèches, Échap, etc.).
        {
            let sessions = sessions.clone();
            let sid = session_id.clone();
            let bracketed = plan.clip.bracketed.clone();
            let multi = state.m7.multi_exec.clone();
            let mut encoder = plan.encoding.new_encoder();
            tauri::async_runtime::spawn(async move {
                while let Some(cmd) = cmd_rx.recv().await {
                    let local = matches!(cmd, SessionCmd::InputLocal(_));
                    let text = match cmd {
                        SessionCmd::Input(text) | SessionCmd::InputLocal(text) => text,
                        SessionCmd::Paste(text) => {
                            // Un marqueur de fin dans le contenu terminerait le
                            // bracketed paste prématurément (pastejacking) : retiré.
                            let clean = text.replace("\x1b[201~", "").replace("\x1b[200~", "");
                            if bracketed.load(std::sync::atomic::Ordering::SeqCst) {
                                format!("\x1b[200~{clean}\x1b[201~")
                            } else {
                                // Comme VTE : fins de ligne → CR (touche Entrée).
                                clean.replace("\r\n", "\r").replace('\n', "\r")
                            }
                        }
                        SessionCmd::Resize(c, r) => {
                            if let Err(e) = sessions.resize(&sid, c, r).await {
                                tracing::warn!(session = %sid, "resize SSH : {e}");
                            }
                            continue;
                        }
                    };
                    let mut buf = vec![0u8; text.len() * 4 + 16];
                    let (_r, _rd, written, _u) = encoder.encode_from_utf8(&text, &mut buf, false);
                    buf.truncate(written);
                    // Multi-exec : les mêmes octets partent vers les autres membres
                    // (jamais pour une frappe locale ; la copie miroir ne repasse
                    // pas par ce chemin, donc aucune rediffusion en boucle).
                    let result = if local {
                        sessions.write(&sid, buf).await
                    } else {
                        haraka_ssh::write_routed(&sessions, &multi, &sid, buf).await
                    };
                    if let Err(e) = result {
                        tracing::warn!(session = %sid, "écriture SSH : {e}");
                    }
                }
            });
        }

        tracing::info!(
            session = %session_id,
            host = %plan.req.target.host,
            port = plan.req.target.port,
            hops = plan.req.hops.len(),
            cols,
            rows,
            "ouverture de session SSH (VTE natif)"
        );
        io.lock().await.insert(
            session_id.clone(),
            SessionIo {
                clip: plan.clip.clone(),
                input: crate::state::InputHandle {
                    broadcast: {
                        let tx = cmd_tx.clone();
                        Arc::new(move |t: String| {
                            let _ = tx.send(SessionCmd::Input(t));
                        })
                    },
                    local: {
                        let tx = cmd_tx.clone();
                        Arc::new(move |t: String| {
                            let _ = tx.send(SessionCmd::InputLocal(t));
                        })
                    },
                },
            },
        );
        meta.lock().await.insert(
            session_id.clone(),
            SessionMeta {
                connection_id,
                title: plan.title,
            },
        );
        let (out_tx, out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let log = open_session_log(&state, &session_id, plan.log_name.as_deref()).await;
        spawn_output_forwarder(
            out_rx,
            OutputSink::Vte {
                ptr: win.vte_ptr,
                alive: win.alive.clone(),
                buf: Arc::new(std::sync::Mutex::new(String::new())),
                in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            plan.encoding,
            plan.clip,
            log,
            highlight,
        );
        if let Err(e) = state.sessions.open(plan.req, out_tx).await {
            crate::vte::unregister(&session_id);
            crate::vte::close_window(&win);
            io.lock().await.remove(&session_id);
            meta.lock().await.remove(&session_id);
            return Err(e.to_string());
        }
        if let Some(app) = state.app_handle.get() {
            use tauri::Emitter;
            let _ = app.emit(
                "session://opened",
                serde_json::json!({ "session": session_id }),
            );
        }
        Ok(session_id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state, connection_id);
        Err("le moteur VTE natif requiert Linux".into())
    }
}

/// Ouvre le journal de la session si la connexion l'a demandé. Un échec
/// (disque, droits) n'empêche pas la session : il est seulement journalisé.
async fn open_session_log(
    state: &AppState,
    session_id: &str,
    name: Option<&str>,
) -> Option<haraka_core::sessionlog::SessionLog> {
    let name = name?;
    // Chiffrement (défaut) : sous-clé dérivée de la DEK, copiée le temps de la création seulement.
    let dek = {
        let inner = state.inner.lock().await;
        inner.vault.as_ref().and_then(|v| {
            v.store
                .settings
                .session_log_encrypt
                .then(|| *v.dek().expose_secret())
        })
    };
    let created = match &dek {
        Some(k) => haraka_core::sessionlog::SessionLog::create_encrypted(&state.data_dir, name, k),
        None => haraka_core::sessionlog::SessionLog::create(&state.data_dir, name),
    };
    match created {
        Ok(log) => {
            let file = log
                .path()
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let _ = haraka_core::audit::append(
                &state.data_dir,
                "session_log_start",
                Some(session_id),
                Some(&format!("file={file}")),
            );
            Some(log)
        }
        Err(e) => {
            tracing::warn!(session = %session_id, "journal de session indisponible : {e}");
            None
        }
    }
}

#[tauri::command]
pub async fn session_close(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    if let Some(app) = state.app_handle.get() {
        crate::multiexec::forget_session(app, &id);
    }
    state.sessions.close(&id).await.map_err(err)?;
    crate::tunnels::stop_session(&state, &id).await;
    state.session_io.lock().await.remove(&id);
    state.session_meta.lock().await.remove(&id);
    Ok(())
}

#[tauri::command]
pub async fn session_reconnect(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.sessions.reconnect(&id).await.map_err(err)
}

#[derive(Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub connection_id: String,
    pub title: String,
    pub state: String,
}

/// Mode onglets : élargit (vue large ouverte) ou rétrécit la colonne du tableau de bord.
#[tauri::command]
pub fn layout_wide(wide: bool) {
    #[cfg(target_os = "linux")]
    crate::vte::set_left_wide(wide);
    #[cfg(not(target_os = "linux"))]
    let _ = wide;
}

#[tauri::command]
pub async fn session_list(state: State<'_, AppState>) -> CmdResult<Vec<SessionInfo>> {
    let states = state.sessions.states().await;
    let meta = state.session_meta.lock().await;
    Ok(meta
        .iter()
        .map(|(id, m)| SessionInfo {
            id: id.clone(),
            connection_id: m.connection_id.clone(),
            title: m.title.clone(),
            state: states.get(id).cloned().unwrap_or_else(|| "inconnu".into()),
        })
        .collect())
}

#[tauri::command]
pub async fn auth_prompt_respond(
    state: State<'_, AppState>,
    prompt_id: String,
    answers: Vec<String>,
) -> CmdResult<()> {
    state
        .sessions
        .respond_auth(&prompt_id, answers)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn hostkey_prompt_respond(
    state: State<'_, AppState>,
    prompt_id: String,
    accept: bool,
    remember: bool,
) -> CmdResult<()> {
    state
        .sessions
        .respond_hostkey(&prompt_id, HostKeyDecision { accept, remember })
        .await
        .map_err(err)?;
    let _ = haraka_core::audit::append(
        &state.data_dir,
        if accept {
            "hostkey_accept"
        } else {
            "hostkey_reject"
        },
        Some(&prompt_id),
        Some(&format!("remember={remember}")),
    );
    Ok(())
}

#[derive(Serialize)]
pub struct KnownHostDto {
    pub host_field: String,
    pub key_type: String,
    pub fingerprint: String,
}

#[tauri::command]
pub async fn known_hosts_list(state: State<'_, AppState>) -> CmdResult<Vec<KnownHostDto>> {
    let entries = knownhosts::list(&state.data_dir.known_hosts_file()).map_err(err)?;
    Ok(entries
        .into_iter()
        .map(|e| KnownHostDto {
            host_field: e.host_field,
            key_type: e.key_type,
            fingerprint: e.fingerprint,
        })
        .collect())
}

/// Oubli explicite d'une entrée (seul moyen de remplacer une clé d'hôte changée).
#[tauri::command]
pub async fn known_hosts_forget(
    state: State<'_, AppState>,
    host_field: String,
    fingerprint: String,
) -> CmdResult<usize> {
    let n = knownhosts::forget(
        &state.data_dir.known_hosts_file(),
        &host_field,
        &fingerprint,
    )
    .map_err(err)?;
    let _ = haraka_core::audit::append(
        &state.data_dir,
        "known_hosts_forget",
        None,
        Some(&format!("entries={n}")),
    );
    Ok(n)
}

/// Efface TOUT le known_hosts interne de l'application (jamais `~/.ssh/known_hosts`) ;
/// sauvegarde `known_hosts.BAK` écrite avant. Chaque hôte repassera par un nouveau TOFU.
#[tauri::command]
pub async fn known_hosts_clear(state: State<'_, AppState>) -> CmdResult<usize> {
    let n = knownhosts::clear(&state.data_dir.known_hosts_file()).map_err(err)?;
    let _ = haraka_core::audit::append(
        &state.data_dir,
        "known_hosts_clear",
        None,
        Some(&format!("entries={n}")),
    );
    Ok(n)
}

#[tauri::command]
pub async fn known_hosts_import_system(state: State<'_, AppState>) -> CmdResult<usize> {
    let home = std::env::var("HOME").map_err(|_| "HOME introuvable")?;
    let src = std::path::PathBuf::from(home)
        .join(".ssh")
        .join("known_hosts");
    let n = knownhosts::import_system(&src, &state.data_dir.known_hosts_file()).map_err(err)?;
    let _ = haraka_core::audit::append(
        &state.data_dir,
        "known_hosts_import",
        None,
        Some(&format!("entries={n}")),
    );
    Ok(n)
}

/// Copie un texte non secret dans le presse-papier (chemin distant, snippet) :
/// pas de TTL d'effacement (contrairement aux secrets, cf. `clipboard_copy_secret`).
#[tauri::command]
pub async fn clipboard_copy_text(text: String) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|e| format!("presse-papier indisponible : {e}"))?;
        clipboard
            .set_text(text)
            .map_err(|e| format!("copie impossible : {e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Pont événements sessions (démarré dans setup) : les événements des sessions
/// natives deviennent dialogues / titres / barre d'état GTK. Un événement pour
/// une fenêtre déjà fermée est simplement ignoré.
pub fn spawn_event_forwarder(
    mut rx: mpsc::UnboundedReceiver<haraka_ssh::SessionEvent>,
    app: AppHandle,
) {
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            #[cfg(target_os = "linux")]
            let _ = forward_native(&app, &ev);
            #[cfg(not(target_os = "linux"))]
            let _ = (&app, &ev);
        }
    });
}

/// Traite un événement pour une session VTE native. Retourne false si aucune
/// fenêtre n'est enregistrée pour cette session (fermée entre-temps).
#[cfg(target_os = "linux")]
fn forward_native(app: &AppHandle, ev: &haraka_ssh::SessionEvent) -> bool {
    use haraka_ssh::SessionEvent as E;
    use tauri::Manager;

    let sid = match ev {
        E::State { session, .. }
        | E::Banner { session, .. }
        | E::AuthPrompt { session, .. }
        | E::HostKeyPrompt { session, .. }
        | E::ExitStatus { session, .. }
        | E::Closed { session, .. } => session,
    };
    let Some(win) = crate::vte::get(sid) else {
        return false;
    };
    let state = app.state::<AppState>();
    match ev {
        E::AuthPrompt {
            prompt_id,
            name,
            instructions,
            prompts,
            ..
        } => {
            let sessions = state.sessions.clone();
            let pid = prompt_id.clone();
            crate::vte::show_auth_prompt(
                win,
                name.clone(),
                instructions.clone(),
                prompts.clone(),
                Box::new(move |answers| {
                    let sessions = sessions.clone();
                    let pid = pid.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = sessions.respond_auth(&pid, answers).await;
                    });
                }),
            );
        }
        E::HostKeyPrompt {
            prompt_id,
            host,
            port,
            kind,
            fingerprint,
            old_fingerprint,
            ..
        } => {
            let sessions = state.sessions.clone();
            let pid = prompt_id.clone();
            let dd = state.data_dir.clone();
            crate::vte::show_hostkey_prompt(
                win,
                host.clone(),
                *port,
                kind == "changed",
                fingerprint.clone(),
                old_fingerprint.clone(),
                Box::new(move |accept, remember| {
                    let sessions = sessions.clone();
                    let pid = pid.clone();
                    let dd = dd.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = sessions
                            .respond_hostkey(&pid, HostKeyDecision { accept, remember })
                            .await;
                        let _ = haraka_core::audit::append(
                            &dd,
                            if accept {
                                "hostkey_accept"
                            } else {
                                "hostkey_reject"
                            },
                            Some(&pid),
                            Some(&format!("remember={remember}")),
                        );
                    });
                }),
            );
        }
        E::State {
            state: st, detail, ..
        } => {
            let l = &win.locale;
            let state_label = crate::i18n::tr(l, &format!("native.state.{st}"));
            // Clé inconnue (état non prévu) : on affiche l'état brut.
            let state_label = if state_label.starts_with("native.state.") {
                st.clone()
            } else {
                state_label
            };
            let suffix = match detail {
                Some(d) => format!("{state_label} — {d}"),
                None => state_label,
            };
            use tauri::Emitter;
            // L'interface rafraîchit sa liste de sessions sur cet événement (plus de polling).
            let _ = app.emit("session://state", serde_json::json!({ "session": sid }));
            crate::vte::set_title(&win, suffix.clone());
            crate::vte::set_status(&win, suffix);
            crate::vte::set_dot(
                &win,
                match st.as_str() {
                    "connected" => "#2e9d4f",
                    "error" | "failed" | "closed" => "#c62828",
                    _ => "#d9a400",
                },
            );
            // Une erreur doit être VISIBLE dans le terminal, pas seulement
            // dans le titre (sinon la fenêtre paraît vide au reconnect).
            if st == "error" {
                crate::vte::feed(
                    &win,
                    format!(
                        "\r\n\x1b[31m{}\x1b[0m\r\n",
                        crate::i18n::tr_with(
                            l,
                            "native.term.connError",
                            &[(
                                "detail",
                                detail
                                    .clone()
                                    .unwrap_or_else(|| crate::i18n::tr(l, "native.term.unknown"))
                                    .as_str()
                            )]
                        )
                    ),
                );
            }
            // Actions post-connexion (suivi OSC 7, tunnels auto, snippet de démarrage).
            if st == "connected" {
                let app = app.clone();
                let sid = sid.clone();
                tauri::async_runtime::spawn(crate::post_connect::run(app, sid));
            }
        }
        E::Banner { text, .. } => {
            crate::vte::feed(&win, format!("\r\n\x1b[33m{text}\x1b[0m\r\n"));
        }
        E::ExitStatus { code, .. } => {
            let code = code.to_string();
            crate::vte::feed(
                &win,
                format!(
                    "\r\n\x1b[33m{}\x1b[0m\r\n",
                    crate::i18n::tr_with(&win.locale, "native.term.exited", &[("code", &code)])
                ),
            );
            crate::vte::set_status(
                &win,
                crate::i18n::tr_with(&win.locale, "native.status.exited", &[("code", &code)]),
            );
        }
        E::Closed { reason, .. } => {
            crate::multiexec::forget_session(app, sid);
            {
                let (app, sid) = (app.clone(), sid.clone());
                tauri::async_runtime::spawn(async move {
                    crate::tunnels::stop_session(&app.state::<AppState>(), &sid).await;
                });
            }
            crate::vte::feed(
                &win,
                format!(
                    "\r\n\x1b[31m{}\x1b[0m\r\n",
                    crate::i18n::tr_with(&win.locale, "native.term.closed", &[("reason", reason)])
                ),
            );
            crate::vte::set_dot(&win, "#c62828");
            crate::vte::set_title(&win, crate::i18n::tr(&win.locale, "native.state.closed"));
            crate::vte::set_status(
                &win,
                crate::i18n::tr_with(&win.locale, "native.status.closed", &[("reason", reason)]),
            );
        }
    }
    true
}

/// Ramène une fenêtre VTE au premier plan (liste « sessions ouvertes »).
#[tauri::command]
pub async fn session_focus_vte(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    #[cfg(target_os = "linux")]
    {
        let _ = state;
        let win = crate::vte::get(&id).ok_or("session native introuvable")?;
        crate::vte::present(&win);
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state, id);
        Err("le moteur VTE natif requiert Linux".into())
    }
}

/// Verrouillage SANS fermer les sessions (réglage `disconnect_on_lock` désactivé) : les fichiers
/// distants déchiffrés sont effacés, le cache du menu de macros vidé et les terminaux MASQUÉS
/// (réaffichés au déverrouillage). Les sessions SSH et leurs tunnels restent ouverts ; les secrets déjà utilisés par les connexions
/// restent en mémoire du gestionnaire de sessions (compromis assumé, avertissement dans les Réglages).
pub async fn lock_keep_sessions(state: &AppState) {
    crate::commands::clear_clipboard_now();
    state.edits.close_all().await;
    state.m7.macro_menu.lock().unwrap().clear();
    // Écrans des terminaux masqués tant que le coffre est verrouillé (réaffichés au déverrouillage).
    #[cfg(target_os = "linux")]
    crate::vte::set_all_visible(false);
}

/// Fermeture de toutes les sessions (verrouillage du coffre).
pub async fn close_all_sessions(state: &AppState) {
    crate::commands::clear_clipboard_now();
    state.sessions.close_all().await;
    crate::tunnels::stop_everything(state).await;
    // Verrouillage : plus aucun fichier distant déchiffré ne doit rester sur le disque.
    state.edits.close_all().await;
    if let Some(app) = state.app_handle.get() {
        crate::multiexec::reset(app);
    }
    state.m7.macro_menu.lock().unwrap().clear();
    state.m7.startup_done.lock().unwrap().clear();
    state.session_io.lock().await.clear();
    state.session_meta.lock().await.clear();
    #[cfg(target_os = "linux")]
    crate::vte::close_all();
}

/// Utilisé par main.rs pour créer le manager + son pont d'événements.
pub fn new_manager() -> (
    Arc<SessionManager>,
    mpsc::UnboundedReceiver<haraka_ssh::SessionEvent>,
) {
    let (manager, rx) = SessionManager::new();
    (Arc::new(manager), rx)
}

#[cfg(test)]
mod tests {
    /// Reproduit le bug du terminal vide : decode_to_string avec un String
    /// de capacité 0 n'émet rien ; avec capacité pré-allouée, tout sort.
    #[test]
    fn decode_utf8_terminal_chunks() {
        let mut decoder = encoding_rs::UTF_8.new_decoder_without_bom_handling();
        let mut out = String::new();
        for chunk in [
            b"Hello ".as_slice(),
            b"world\r\n\x1b[32m$ \x1b[0m".as_slice(),
        ] {
            let cap = decoder
                .max_utf8_buffer_length(chunk.len())
                .unwrap_or(chunk.len() * 3 + 8);
            let mut text = String::with_capacity(cap.max(8));
            let (result, read, _errs) = decoder.decode_to_string(chunk, &mut text, false);
            assert_eq!(result, encoding_rs::CoderResult::InputEmpty);
            assert_eq!(read, chunk.len());
            assert!(!text.is_empty(), "chunk avalé silencieusement");
            out.push_str(&text);
        }
        assert!(out.contains("Hello world"));
        assert!(out.contains("\x1b[32m"));
    }

    #[test]
    fn zero_capacity_string_swallows_everything() {
        // Preuve du bug d'origine : String::new() → 0 caractère émis.
        let mut decoder = encoding_rs::UTF_8.new_decoder_without_bom_handling();
        let mut text = String::new();
        let (_result, _read, _errs) = decoder.decode_to_string(b"Hello world", &mut text, false);
        assert!(text.is_empty(), "ce test documente le bug corrigé");
    }
}

// --- Journaux de session : liste, lecture (déchiffrée), suppression, purge ---------------

#[derive(Serialize)]
pub struct LogFileDto {
    pub name: String,
    pub size: u64,
    pub modified_epoch: Option<u64>,
    pub encrypted: bool,
}

fn log_file_name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\', '\0'])
        && name != "."
        && name != ".."
        && (name.ends_with(".log") || name.ends_with(".log.enc"))
}

#[tauri::command]
pub async fn session_logs_list(state: State<'_, AppState>) -> CmdResult<Vec<LogFileDto>> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(state.data_dir.logs_dir()) else {
        return Ok(out);
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !log_file_name_ok(&name) {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        out.push(LogFileDto {
            encrypted: name.ends_with(".enc"),
            size: meta.len(),
            modified_epoch: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
            name,
        });
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.modified_epoch));
    Ok(out)
}

/// Contenu d'un journal (déchiffré si besoin), tronqué aux derniers 2 Mo pour l'affichage.
#[tauri::command]
pub async fn session_log_read(state: State<'_, AppState>, name: String) -> CmdResult<String> {
    if !log_file_name_ok(&name) {
        return Err("nom de journal invalide".into());
    }
    let path = state.data_dir.logs_dir().join(&name);
    let text = if name.ends_with(".enc") {
        let dek = {
            let inner = state.inner.lock().await;
            *inner
                .vault
                .as_ref()
                .ok_or(crate::commands::LOCKED)?
                .dek()
                .expose_secret()
        };
        haraka_core::sessionlog::decrypt_log(&path, &dek).map_err(err)?
    } else {
        String::from_utf8_lossy(&std::fs::read(&path).map_err(|e| e.to_string())?).into_owned()
    };
    const MAX: usize = 2 * 1024 * 1024;
    if text.len() > MAX {
        let mut start = text.len() - MAX;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        return Ok(text[start..].to_string());
    }
    Ok(text)
}

#[tauri::command]
pub async fn session_log_delete(state: State<'_, AppState>, name: String) -> CmdResult<()> {
    if !log_file_name_ok(&name) {
        return Err("nom de journal invalide".into());
    }
    std::fs::remove_file(state.data_dir.logs_dir().join(&name)).map_err(|e| e.to_string())?;
    let _ = haraka_core::audit::append(&state.data_dir, "session_log_delete", None, Some(&name));
    Ok(())
}

/// Purge manuelle selon la rétention des réglages (ou `days` si fourni).
#[tauri::command]
pub async fn session_logs_purge(state: State<'_, AppState>, days: Option<u32>) -> CmdResult<usize> {
    let days = match days {
        Some(d) => d,
        None => {
            let inner = state.inner.lock().await;
            inner
                .vault
                .as_ref()
                .ok_or(crate::commands::LOCKED)?
                .store
                .settings
                .session_log_retention_days
        }
    };
    let n = haraka_core::sessionlog::purge_older_than(&state.data_dir.logs_dir(), days);
    let _ = haraka_core::audit::append(
        &state.data_dir,
        "session_logs_purge",
        None,
        Some(&format!("days={days} removed={n}")),
    );
    Ok(n)
}

#[derive(Serialize)]
pub struct AgentKeyDto {
    pub algorithm: String,
    pub fingerprint: String,
    pub comment: String,
    pub certificate: bool,
    /// Clé matérielle FIDO2 (`sk-…`) : la signature reste dans le jeton.
    pub hardware: bool,
}

#[derive(Serialize)]
pub struct AgentStatusDto {
    pub available: bool,
    pub keys: Vec<AgentKeyDto>,
}

/// État de l'agent SSH local (lecture seule, aucun secret).
#[tauri::command]
pub async fn ssh_agent_status() -> CmdResult<AgentStatusDto> {
    Ok(match haraka_ssh::agent_keys().await {
        None => AgentStatusDto {
            available: false,
            keys: Vec::new(),
        },
        Some(keys) => AgentStatusDto {
            available: true,
            keys: keys
                .into_iter()
                .map(|k| AgentKeyDto {
                    hardware: k.algorithm.starts_with("sk-"),
                    algorithm: k.algorithm,
                    fingerprint: k.fingerprint,
                    comment: k.comment,
                    certificate: k.certificate,
                })
                .collect(),
        },
    })
}
