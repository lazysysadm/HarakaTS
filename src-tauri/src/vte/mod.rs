//! Terminal natif GTK+VTE — le moteur de Remmina (jalon M3-VTE).
//!
//! Pourquoi VTE : le rendu xterm.js dans la webview WebKitGTK figeait
//! htop/tmux (process GPU + inondation IPC). VTE est un widget GTK natif :
//! rendu, clavier, resize et copier/coller gérés par le système.
//!
//! FFI direct sur libvte-2.91 (API C stable depuis 0.40, LGPL) : les bindings
//! Rust existants (vte-rs) sont GPL-3.0 ou basés sur un GTK incompatible avec
//! celui de Tauri (conflit `links = "gtk-3"`).
//!
//! Mode « feed » sans PTY local : la sortie SSH est décodée (encoding_rs)
//! puis injectée via `Term::feed` ; le clavier part par le signal « commit » ;
//! le resize par « size-allocate » (lu en idle, grille à jour). Toutes les
//! calls GTK/VTE ont lieu sur le thread principal (contexte glib), jamais
//! depuis tokio.
//!
//! Organisation :
//! - `ffi`       : FFI libvte/pango + wrapper sûr `Term` (seul `unsafe` du module)
//! - `handle`    : `VteWindow` (poignée Send+Sync), registre, titre/statut/feed
//! - `window`    : assemblage de la fenêtre (layout, câblage)
//! - `clipboard` : raccourcis, clic milieu, menu contextuel, collage confirmé
//! - `resize`    : dimensions initiales du PTY et resize débouncé
//! - `dialogs`   : auth keyboard-interactive, clé d'hôte, confirmation de collage

mod clipboard;
mod dialogs;
mod ffi;
mod handle;
mod resize;
mod tabs;
mod window;

use std::sync::Arc;
use std::time::Duration;

use gtk::glib;
use tokio::sync::oneshot;

pub use dialogs::{ask_macro_confirm, show_auth_prompt, show_hostkey_prompt};
pub use handle::{
    VteWindow, begin_conceal, close_all, close_window, feed, get, present, register, schedule_feed,
    set_all_visible, set_dot, set_left_wide, set_multiexec, set_status, set_title, unregister,
};

/// Rappels vers le backend (exécutés depuis le thread GTK : doivent être courts
/// et non bloquants — ils postent dans des canaux).
pub struct VteCallbacks {
    /// Texte clavier UTF-8 (signal « commit »).
    pub on_input: Arc<dyn Fn(String) + Send + Sync>,
    /// Collage explicite (Ctrl+Shift+V, clic milieu, menu), déjà confirmé si suspect.
    pub on_paste: Arc<dyn Fn(String) + Send + Sync>,
    /// Nouvelles dimensions (debounce 100 ms appliqué ici).
    pub on_resize: Arc<dyn Fn(u32, u32) + Send + Sync>,
    /// Fenêtre fermée par l'utilisateur (delete-event).
    pub on_close: Arc<dyn Fn() + Send + Sync>,
    /// Bouton « Reconnecter » de la barre d'état.
    pub on_reconnect: Arc<dyn Fn() + Send + Sync>,
    /// Fenêtre devenue active (focus) : le panneau SFTP suit cette session.
    pub on_focus: Arc<dyn Fn() + Send + Sync>,
    /// Bouton « Fichiers (SFTP) » : ouvre le panneau SFTP de cette session.
    pub on_sftp: Arc<dyn Fn() + Send + Sync>,
    /// Entrées du sous-menu « Macros » : (id, nom, confirmation requise).
    /// Appelé à chaque clic droit ; ne renvoie jamais le contenu.
    pub macro_entries: MacroEntries,
    /// Macro choisie dans le menu (id) : l'envoi et la confirmation sont gérés côté backend.
    pub on_macro: Arc<dyn Fn(String) + Send + Sync>,
}

/// Fournisseur des entrées du menu « Macros » (id, nom, confirmation requise).
pub type MacroEntries = Arc<dyn Fn() -> Vec<(String, String, bool)> + Send + Sync>;

/// Réglages de la fenêtre terminal (issus des réglages de l'application).
#[derive(Clone)]
pub struct VteOptions {
    pub font_size: u32,
    pub scrollback: u32,
    pub copy_on_select: bool,
    pub paste_confirm: bool,
    /// Langue des textes natifs (menus, boutons, dialogues) de cette fenêtre.
    pub locale: String,
    /// `Some(adresse de la fenêtre principale)` : terminal en onglet dans cette fenêtre.
    pub main_window_ptr: Option<usize>,
}

/// Délai max d'attente des premières dimensions (fenêtre non affichée ?).
const FIRST_DIMS_TIMEOUT: Duration = Duration::from_secs(5);

/// Crée la fenêtre sur le thread principal GTK et attend ses premières
/// dimensions réelles : le PTY serveur doit être ouvert à la bonne taille.
pub async fn open_window(
    session_id: String,
    title: String,
    cb: VteCallbacks,
    opts: VteOptions,
) -> Result<(VteWindow, u32, u32), String> {
    let (win_tx, win_rx) = oneshot::channel::<VteWindow>();
    let (dim_tx, dim_rx) = oneshot::channel::<(u32, u32)>();
    // Si le contexte GTK n'est pas disponible, la closure ne s'exécute pas
    // et win_rx échoue ci-dessous.
    glib::MainContext::default().invoke(move || {
        match window::create(&session_id, &title, cb, dim_tx, &opts) {
            Ok(win) => {
                let _ = win_tx.send(win);
            }
            Err(e) => tracing::error!("création fenêtre VTE impossible : {e}"),
        }
    });
    let win = win_rx
        .await
        .map_err(|_| "création de la fenêtre VTE impossible (contexte GTK)".to_string())?;
    let (cols, rows) = match tokio::time::timeout(FIRST_DIMS_TIMEOUT, dim_rx).await {
        Ok(Ok(dims)) => dims,
        _ => {
            close_window(&win);
            return Err("dimensions du terminal VTE non obtenues (fenêtre non affichée ?)".into());
        }
    };
    Ok((win, cols, rows))
}
