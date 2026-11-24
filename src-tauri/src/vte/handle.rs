//! Poignée thread-safe vers une fenêtre VTE + registre des sessions natives.
//!
//! Les fonctions publiques sont appelables depuis tokio : elles reviennent sur
//! le thread principal GTK via `on_main_if_alive`, qui vérifie `alive` juste
//! avant d'exécuter (destruction et accès ont lieu sur le même thread : pas
//! de course entre le test et l'usage).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use gtk::glib;
use gtk::prelude::*;

use super::ffi::{Term, label_from_addr, window_from_addr};

/// Poignée clonable (Send + Sync) : adresses des widgets + drapeau de vie.
#[derive(Clone)]
pub struct VteWindow {
    pub alive: Arc<AtomicBool>,
    pub vte_ptr: usize,
    pub window_ptr: usize,
    pub status_ptr: usize,
    /// Étiquette « MULTI-EXEC (n) » de la barre d'état (masquée hors multi-exec).
    pub multiexec_ptr: usize,
    pub title_base: String,
    /// Identifiant de session (mode onglets : fermeture sans delete-event).
    pub session_id: String,
    /// Adresse de la page dans le notebook (0 = fenêtre indépendante).
    pub page_ptr: usize,
    /// Fermeture de la session (mode onglets).
    pub on_close: Arc<dyn Fn() + Send + Sync>,
    /// Langue des textes natifs de la fenêtre (fixée à la création).
    pub locale: String,
}

/// Exécute `f` sur le thread GTK si la fenêtre est toujours vivante.
fn on_main_if_alive(alive: &Arc<AtomicBool>, f: impl FnOnce() + Send + 'static) {
    let alive = alive.clone();
    glib::MainContext::default().invoke(move || {
        if alive.load(Ordering::SeqCst) {
            f();
        }
    });
}

// ---------------------------------------------------------------- registre

static REGISTRY: OnceLock<Mutex<HashMap<String, VteWindow>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, VteWindow>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register(id: String, win: VteWindow) {
    registry().lock().unwrap().insert(id, win);
}

pub fn unregister(id: &str) {
    registry().lock().unwrap().remove(id);
}

pub fn get(id: &str) -> Option<VteWindow> {
    registry().lock().unwrap().get(id).cloned()
}

/// Masque ou ré-affiche tous les terminaux sans fermer les sessions : fenêtres indépendantes
/// (`hide`/`show`) et notebook du mode onglets. Utilisé au verrouillage / déverrouillage du coffre.
pub fn set_all_visible(visible: bool) {
    let wins: Vec<VteWindow> = registry().lock().unwrap().values().cloned().collect();
    glib::MainContext::default().invoke(move || {
        for win in wins {
            if !win.alive.load(Ordering::SeqCst) || win.page_ptr != 0 {
                continue;
            }
            let window = unsafe { window_from_addr(win.window_ptr) };
            if visible {
                window.show();
            } else {
                window.hide();
            }
        }
        super::tabs::set_visible(visible);
    });
}

/// Ferme toutes les fenêtres natives (verrouillage du coffre).
pub fn close_all() {
    let wins: Vec<VteWindow> = registry().lock().unwrap().drain().map(|(_, w)| w).collect();
    for win in wins {
        close_window(&win);
    }
}

// ---------------------------------------------------------------- fenêtre

/// Ferme la fenêtre (`alive` reste true jusqu'au delete-event, sinon
/// `on_main_if_alive` sortirait sans fermer).
pub fn close_window(win: &VteWindow) {
    let ptr = win.window_ptr;
    if win.page_ptr != 0 {
        let (sid, alive, page, on_close) = (
            win.session_id.clone(),
            win.alive.clone(),
            win.page_ptr,
            win.on_close.clone(),
        );
        on_main_if_alive(&win.alive.clone(), move || {
            close_tab(&sid, &alive, page, &on_close);
        });
        return;
    }
    on_main_if_alive(&win.alive, move || {
        unsafe { window_from_addr(ptr) }.close();
    });
}

/// Fermeture d'un onglet : même nettoyage que le delete-event d'une fenêtre.
pub fn close_tab(
    session_id: &str,
    alive: &Arc<AtomicBool>,
    page_ptr: usize,
    on_close: &Arc<dyn Fn() + Send + Sync>,
) {
    tracing::info!(session = %session_id, "onglet terminal fermé");
    alive.store(false, Ordering::SeqCst);
    unregister(session_id);
    super::tabs::remove_page(page_ptr);
    on_close();
}

/// Ramène la fenêtre au premier plan (liste « sessions ouvertes »).
pub fn present(win: &VteWindow) {
    let ptr = win.window_ptr;
    let page = win.page_ptr;
    on_main_if_alive(&win.alive, move || {
        if page != 0 {
            super::tabs::select(page);
        }
        unsafe { window_from_addr(ptr) }.present();
    });
}

pub fn set_title(win: &VteWindow, suffix: String) {
    let ptr = win.window_ptr;
    let page = win.page_ptr;
    if page != 0 {
        // Onglet : le titre reste le nom de la connexion, l'état se lit sur le point coloré.
        return;
    }
    let title = format!("HarakaTS — {} — {suffix}", win.title_base);
    on_main_if_alive(&win.alive, move || {
        unsafe { window_from_addr(ptr) }.set_title(&title);
    });
}

/// Point d'état de l'onglet (no-op pour une fenêtre indépendante).
pub fn set_dot(win: &VteWindow, color: &'static str) {
    let page = win.page_ptr;
    if page != 0 {
        on_main_if_alive(&win.alive, move || super::tabs::set_dot(page, color));
    }
}

/// Largeur du tableau de bord en mode onglets (vue large ou colonne étroite).
pub fn set_left_wide(wide: bool) {
    glib::MainContext::default().invoke(move || super::tabs::set_left_wide(wide));
}

pub fn set_status(win: &VteWindow, text: String) {
    let ptr = win.status_ptr;
    on_main_if_alive(&win.alive, move || {
        unsafe { label_from_addr(ptr) }.set_text(&text);
    });
}

/// Indicateur permanent de multi-exec dans la barre d'état : `Some(n)` l'affiche
/// (rouge, gras, n = nombre de membres), `None` le masque.
pub fn set_multiexec(win: &VteWindow, members: Option<usize>) {
    let ptr = win.multiexec_ptr;
    let text = members.map(|n| {
        crate::i18n::tr_with(
            &win.locale,
            "native.multiexec.badge",
            &[("n", &n.to_string())],
        )
    });
    on_main_if_alive(&win.alive, move || {
        let label = unsafe { label_from_addr(ptr) };
        match text {
            Some(t) => {
                label.set_markup(&format!(
                    "<span foreground=\"#ffffff\" background=\"#c62828\" weight=\"bold\"> {} </span>",
                    glib::markup_escape_text(&t)
                ));
                label.show();
            }
            None => {
                label.set_text("");
                label.hide();
            }
        }
    });
}

// ---------------------------------------------------------------- rendu

/// Injecte du texte UTF-8 décodé dans le terminal (thread principal GTK).
pub fn feed_ptr(ptr: usize, alive: &Arc<AtomicBool>, text: String) {
    if text.is_empty() {
        return;
    }
    let alive_check = alive.clone();
    glib::MainContext::default().invoke(move || {
        if !alive_check.load(Ordering::SeqCst) {
            tracing::warn!("feed VTE ignoré : fenêtre détruite");
            return;
        }
        unsafe { Term::from_addr(ptr) }.feed(&text);
    });
}

pub fn feed(win: &VteWindow, text: String) {
    feed_ptr(win.vte_ptr, &win.alive, text);
}

/// Feed coalescé pour la sortie à haut débit : un seul invoke en vol par
/// session, qui vide un tampon partagé. Si le thread GTK est occupé (reflow
/// d'un VTE agrandi), les lots s'accumulent dans le tampon au lieu d'empiler
/// des invokes — pas de saturation de la boucle principale.
pub fn schedule_feed(
    ptr: usize,
    alive: &Arc<AtomicBool>,
    buf: &Arc<Mutex<String>>,
    in_flight: &Arc<AtomicBool>,
) {
    if in_flight.swap(true, Ordering::SeqCst) {
        return;
    }
    let alive = alive.clone();
    let buf = buf.clone();
    let in_flight = in_flight.clone();
    glib::MainContext::default().invoke(move || {
        let text = std::mem::take(&mut *buf.lock().unwrap());
        if alive.load(Ordering::SeqCst) && !text.is_empty() {
            unsafe { Term::from_addr(ptr) }.feed(&text);
        }
        in_flight.store(false, Ordering::SeqCst);
        // Du texte a pu arriver pendant le feed → re-planifier (un seul vol).
        if !buf.lock().unwrap().is_empty() {
            schedule_feed(ptr, &alive, &buf, &in_flight);
        }
    });
}

/// Masque la prochaine saisie automatique de `echo_len` caractères (attribut
/// SGR « conceal »). Retourne le nombre de lignes que son écho occupera
/// (selon la colonne du curseur et la largeur de la grille) : ce nombre sert à
/// effacer la ligne après coup, par mouvements RELATIFS (sûrs même si l'écran
/// défile). Doit être appelé AVANT l'envoi de la saisie.
pub fn begin_conceal(win: &VteWindow, echo_len: usize) -> tokio::sync::oneshot::Receiver<u32> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let ptr = win.vte_ptr;
    on_main_if_alive(&win.alive, move || {
        let term = unsafe { Term::from_addr(ptr) };
        let (col, _) = term.cursor_position();
        let (cols, _) = term.grid_size();
        let cols = cols.max(1) as usize;
        let rows = (col as usize + echo_len).div_ceil(cols).max(1) as u32;
        term.feed("\x1b[8m");
        let _ = tx.send(rows);
    });
    rx
}
