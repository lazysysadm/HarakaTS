//! Mode « onglets » : les terminaux sont des pages d'un `gtk::Notebook` placé
//! à droite de la webview, dans la fenêtre principale, séparés par un `gtk::Paned`. Colonne de gauche :
//! étroite (arbre) et réglable à la souris entre 200 et 420 px, ou large et FIXE quand une vue (SFTP,
//! éditeur…) est ouverte. Les positions sont posées avec `Paned::set_position` (réallocation immédiate).
//! Tout s'exécute sur le thread GTK ; le notebook est créé à la première
//! utilisation (aucun effet tant que l'option est désactivée).

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use gtk::prelude::*;

use super::ffi::{widget_from_addr, window_from_addr};

struct Entry {
    page: usize,
    dot: gtk::Label,
    term: gtk::Widget,
    focus: Arc<dyn Fn() + Send + Sync>,
}

thread_local! {
    static NOTEBOOK: RefCell<Option<gtk::Notebook>> = const { RefCell::new(None) };
    static ENTRIES: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    static PANED: RefCell<Option<gtk::Paned>> = const { RefCell::new(None) };
    /// Une vue large est ouverte dans le tableau de bord.
    static WIDE: Cell<bool> = const { Cell::new(false) };
    /// Dernière largeur choisie par l'utilisateur pour la colonne étroite (arbre).
    static NARROW: Cell<i32> = const { Cell::new(NARROW_DEFAULT) };
    /// Au retour en mode étroit : repartir de la largeur mémorisée, pas de la position large.
    static RESTORE_NARROW: Cell<bool> = const { Cell::new(false) };
}

/// Notebook des terminaux ; insère un `Paned` horizontal (webview à gauche, notebook à droite)
/// dans la boîte principale à la première demande.
pub fn ensure(main_ptr: usize) -> Result<gtk::Notebook, String> {
    if let Some(nb) = NOTEBOOK.with(|n| n.borrow().clone()) {
        return Ok(nb);
    }
    let win = unsafe { window_from_addr(main_ptr) };
    let root = win.child().ok_or("fenêtre principale sans contenu")?;
    let vbox = root
        .downcast::<gtk::Box>()
        .map_err(|_| "conteneur principal inattendu")?;
    let web = vbox
        .children()
        .into_iter()
        .find(|c| c.type_().name().contains("WebView"))
        .ok_or("webview introuvable dans la fenêtre principale")?;

    vbox.remove(&web);
    // Disposition façon mRemoteNG : arbre/tableau de bord à gauche, terminaux (onglets) à droite.
    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_wide_handle(true);
    let nb = gtk::Notebook::new();
    nb.set_scrollable(true);
    nb.set_show_tabs(true);
    nb.set_tab_pos(gtk::PositionType::Top);
    nb.set_no_show_all(true);
    nb.set_size_request(MIN_RIGHT, -1);
    // Plancher natif : le séparateur ne peut pas descendre sous NARROW_MIN (pas d'image intermédiaire).
    web.set_size_request(NARROW_MIN, -1);
    paned.pack1(&web, false, false);
    paned.pack2(&nb, true, false);
    paned.set_position(NARROW_DEFAULT);
    vbox.pack_start(&paned, true, true, 0);
    paned.show();
    // Position modifiée à la souris : on borne (étroit) ou on rétablit (large) IMMÉDIATEMENT, dans le
    // signal, avant le prochain dessin. Corriger plus tard (idle) laissait passer une image hors bornes
    // à chaque mouvement, d'où un scintillement aux limites. Sans risque de boucle : `apply_layout` ne
    // déplace le séparateur que si la position diffère de la cible.
    paned.connect_notify_local(Some("position"), |_, _| apply_layout());
    paned.connect_size_allocate(|_, _| schedule_layout());
    PANED.with(|p| *p.borrow_mut() = Some(paned));

    nb.connect_switch_page(|_, page, _| {
        let addr = page.as_ptr() as usize;
        let hit = ENTRIES.with(|e| {
            e.borrow()
                .iter()
                .find(|x| x.page == addr)
                .map(|x| (x.focus.clone(), x.term.clone()))
        });
        if let Some((focus, term)) = hit {
            focus();
            term.grab_focus();
        }
    });
    NOTEBOOK.with(|n| *n.borrow_mut() = Some(nb.clone()));
    Ok(nb)
}

/// Ajoute une page (terminal + barre d'état) avec un onglet « titre ✕ ».
pub fn add_page(
    nb: &gtk::Notebook,
    page: &gtk::Widget,
    term: &gtk::Widget,
    title: &str,
    focus: Arc<dyn Fn() + Send + Sync>,
    on_close_click: impl Fn() + 'static,
) {
    let tab = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    // Point d'état (vert = connecté, orange = en cours, rouge = erreur/fermé).
    let dot = gtk::Label::new(None);
    dot.set_markup(&dot_markup("#d9a400"));
    let label = gtk::Label::new(Some(title));
    let btn = gtk::Button::with_label("\u{2715}");
    btn.set_relief(gtk::ReliefStyle::None);
    btn.set_focus_on_click(false);
    btn.connect_clicked(move |_| on_close_click());
    tab.pack_start(&dot, false, false, 0);
    tab.pack_start(&label, true, true, 0);
    tab.pack_start(&btn, false, false, 0);
    tab.show_all();
    ENTRIES.with(|e| {
        e.borrow_mut().push(Entry {
            page: page.as_ptr() as usize,
            dot,
            term: term.clone(),
            focus,
        });
    });
    let first = nb.n_pages() == 0;
    let idx = nb.append_page(page, Some(&tab));
    nb.set_tab_reorderable(page, true);
    nb.show();
    if first {
        apply_layout();
    }
    nb.set_current_page(Some(idx));
}

pub fn remove_page(page_ptr: usize) {
    let Some(nb) = NOTEBOOK.with(|n| n.borrow().clone()) else {
        return;
    };
    let page = unsafe { widget_from_addr(page_ptr) };
    ENTRIES.with(|e| e.borrow_mut().retain(|x| x.page != page_ptr));
    if let Some(idx) = nb.page_num(&page) {
        nb.remove_page(Some(idx));
    }
    if nb.n_pages() == 0 {
        nb.hide();
        apply_layout();
    }
}

/// Affiche ou masque tout le notebook des terminaux (verrouillage du coffre sans fermeture des sessions).
pub fn set_visible(visible: bool) {
    let Some(nb) = NOTEBOOK.with(|n| n.borrow().clone()) else {
        return;
    };
    if !visible {
        nb.hide();
    } else if nb.n_pages() > 0 {
        nb.show();
    }
    apply_layout();
}

pub fn select(page_ptr: usize) {
    let Some(nb) = NOTEBOOK.with(|n| n.borrow().clone()) else {
        return;
    };
    let page = unsafe { widget_from_addr(page_ptr) };
    if let Some(idx) = nb.page_num(&page) {
        nb.set_current_page(Some(idx));
    }
}

/// Colonne étroite (arbre) : 240 px par défaut, réglable à la souris entre 200 et 420 px.
const NARROW_DEFAULT: i32 = 240;
const NARROW_MIN: i32 = 200;
const NARROW_MAX: i32 = 420;
const MIN_RIGHT: i32 = 360;

fn dot_markup(color: &str) -> String {
    format!("<span foreground=\"{color}\">\u{25CF}</span>")
}

/// Couleur du point d'état de l'onglet.
pub fn set_dot(page_ptr: usize, color: &str) {
    ENTRIES.with(|e| {
        if let Some(x) = e.borrow().iter().find(|x| x.page == page_ptr) {
            x.dot.set_markup(&dot_markup(color));
        }
    });
}

/// Colonne de gauche (tableau de bord) : large et fixe quand une vue large est ouverte (éditeur, SFTP du
/// tableau de bord, réglages…), étroite et réglable (200–420 px) sinon.
pub fn set_left_wide(wide: bool) {
    if WIDE.with(|w| w.replace(wide)) && !wide {
        RESTORE_NARROW.with(|r| r.set(true));
    }
    apply_layout();
}

/// Replanifie `apply_layout` hors de l'allocation en cours (une seule fois à la fois).
fn schedule_layout() {
    thread_local! {
        static PENDING: Cell<bool> = const { Cell::new(false) };
    }
    if PENDING.with(|p| p.replace(true)) {
        return;
    }
    gtk::glib::idle_add_local_once(|| {
        PENDING.with(|p| p.set(false));
        apply_layout();
    });
}

/// Applique la disposition. Sans terminal visible, la webview occupe tout. Sinon : mode large =
/// position fixe (~55 %, mini 880 px) rétablie si on la déplace ; mode étroit = position choisie à la
/// souris, bornée entre `NARROW_MIN` et `NARROW_MAX`. Les terminaux gardent au moins `MIN_RIGHT`.
fn apply_layout() {
    let (Some(paned), Some(nb)) = (
        PANED.with(|p| p.borrow().clone()),
        NOTEBOOK.with(|n| n.borrow().clone()),
    ) else {
        return;
    };
    if !nb.is_visible() {
        return;
    }
    let total = paned.allocated_width();
    if total <= 0 {
        return;
    }
    let max_left = (total - MIN_RIGHT).max(NARROW_MIN);
    let current = paned.position();
    let target = if WIDE.with(Cell::get) {
        (total * 55 / 100).max(880).min(max_left)
    } else {
        let base = if RESTORE_NARROW.with(|r| r.replace(false)) {
            NARROW.with(Cell::get)
        } else {
            current
        };
        let t = base.clamp(NARROW_MIN, NARROW_MAX).min(max_left);
        NARROW.with(|n| n.set(t));
        t
    };
    if (current - target).abs() > 1 {
        tracing::debug!(
            target,
            current,
            total,
            wide = WIDE.with(Cell::get),
            "colonne gauche"
        );
        paned.set_position(target);
    }
}
