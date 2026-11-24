//! Copier/coller du terminal natif (M4) : raccourcis, clic milieu, menu
//! contextuel, copy-on-select.
//!
//! VTE ne câble PAS Ctrl+Shift+C/V (c'est à l'application). Tout collage
//! (clavier, clic milieu, menu) passe par `paste_from` : lecture du
//! presse-papier GTK, confirmation unique si multiligne/suspect, puis
//! `on_paste`. Les frappes (signal « commit ») ne sont jamais confirmées.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gtk::glib;
use gtk::prelude::*;

use super::dialogs::confirm_paste;
use super::ffi::Term;

/// Réglages presse-papier d'une fenêtre.
pub struct ClipboardOptions {
    /// Langue des textes natifs (menu contextuel, dialogue de collage).
    pub locale: String,
    pub paste_confirm: bool,
    pub copy_on_select: bool,
    pub macro_entries: super::MacroEntries,
    pub on_macro: Arc<dyn Fn(String) + Send + Sync>,
}

/// Actions presse-papier partagées par les raccourcis, le clic milieu et le menu.
#[derive(Clone)]
struct Actions {
    locale: Rc<str>,
    copy: Rc<dyn Fn()>,
    /// `true` = sélection PRIMARY (clic milieu), `false` = CLIPBOARD.
    paste: Rc<dyn Fn(bool)>,
    macro_entries: super::MacroEntries,
    on_macro: Arc<dyn Fn(String) + Send + Sync>,
}

/// Branche tous les comportements presse-papier sur le widget terminal.
pub fn bind(
    widget: &gtk::Widget,
    term: Term,
    window_ptr: usize,
    alive: &Arc<AtomicBool>,
    opts: ClipboardOptions,
    on_paste: Arc<dyn Fn(String) + Send + Sync>,
) {
    let alive = alive.clone();
    let locale: Rc<str> = Rc::from(opts.locale.as_str());
    let paste_locale = opts.locale.clone();
    let paste_confirm = opts.paste_confirm;
    let actions = Actions {
        macro_entries: opts.macro_entries.clone(),
        on_macro: opts.on_macro.clone(),
        locale,
        copy: Rc::new(move || term.copy_selection()),
        paste: Rc::new(move |primary| {
            paste_from(
                primary,
                window_ptr,
                alive.clone(),
                &paste_locale,
                paste_confirm,
                on_paste.clone(),
            )
        }),
    };
    if opts.copy_on_select {
        bind_copy_on_select(widget, term);
    }
    bind_shortcuts(widget, actions.clone());
    bind_mouse(widget, term, actions);
}

/// Copy-on-select : la sélection part aussi dans CLIPBOARD (PRIMARY est
/// native sous GTK).
fn bind_copy_on_select(widget: &gtk::Widget, term: Term) {
    widget.connect_local("selection-changed", false, move |_| {
        if term.has_selection() {
            term.copy_selection();
        }
        None
    });
}

/// Ctrl+Shift+C/V, Ctrl+Inser (copier), Maj+Inser (coller).
fn bind_shortcuts(widget: &gtk::Widget, actions: Actions) {
    widget.connect_key_press_event(move |_, ev| {
        use gtk::gdk::ModifierType as M;
        use gtk::gdk::keys::constants as k;
        let st = ev.state();
        let key = ev.keyval();
        let ctrl = st.contains(M::CONTROL_MASK);
        let shift = st.contains(M::SHIFT_MASK);
        let is_copy =
            ctrl && ((shift && (key == k::C || key == k::c)) || (!shift && key == k::Insert));
        let is_paste =
            (ctrl && shift && (key == k::V || key == k::v)) || (shift && !ctrl && key == k::Insert);
        if is_copy {
            (actions.copy)();
            glib::Propagation::Stop
        } else if is_paste {
            (actions.paste)(false);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
}

/// Clic milieu = collage PRIMARY ; clic droit = menu contextuel.
fn bind_mouse(widget: &gtk::Widget, term: Term, actions: Actions) {
    widget.connect_button_press_event(move |_, ev| match ev.button() {
        2 => {
            (actions.paste)(true);
            glib::Propagation::Stop
        }
        3 => {
            context_menu(term, &actions).popup_at_pointer(Some(ev));
            glib::Propagation::Stop
        }
        _ => glib::Propagation::Proceed,
    });
}

fn context_menu(term: Term, actions: &Actions) -> gtk::Menu {
    let menu = gtk::Menu::new();
    let add = |label: &str, run: Rc<dyn Fn()>| {
        let item = gtk::MenuItem::with_label(label);
        item.connect_activate(move |_| run());
        menu.append(&item);
    };
    let l = |key: &str| crate::i18n::tr(&actions.locale, key);
    add(&l("native.menu.copy"), actions.copy.clone());
    let paste = actions.paste.clone();
    add(&l("native.menu.paste"), Rc::new(move || paste(false)));
    add(
        &l("native.menu.selectAll"),
        Rc::new(move || term.select_all()),
    );
    add(
        &l("native.menu.clearScreen"),
        Rc::new(move || term.reset(false)),
    );
    add(
        &l("native.menu.resetTerminal"),
        Rc::new(move || term.reset(true)),
    );
    let macros = macros_submenu(actions);
    menu.append(&gtk::SeparatorMenuItem::new());
    let macros_item = gtk::MenuItem::with_label(&l("native.macro.menu"));
    macros_item.set_submenu(Some(&macros));
    menu.append(&macros_item);
    menu.show_all();
    menu
}

/// Sous-menu « Macros » : un élément par macro (nom seulement ; « … » = confirmation
/// avant envoi). L'envoi part vers le backend, qui lit le contenu déchiffré.
fn macros_submenu(actions: &Actions) -> gtk::Menu {
    let sub = gtk::Menu::new();
    let entries = (actions.macro_entries)();
    if entries.is_empty() {
        let none =
            gtk::MenuItem::with_label(&crate::i18n::tr(&actions.locale, "native.macro.none"));
        none.set_sensitive(false);
        sub.append(&none);
    }
    for (id, name, confirm) in entries {
        let label = if confirm { format!("{name} …") } else { name };
        let item = gtk::MenuItem::with_label(&label);
        let on_macro = actions.on_macro.clone();
        item.connect_activate(move |_| on_macro(id.clone()));
        sub.append(&item);
    }
    sub
}

/// Lit le presse-papier GTK (CLIPBOARD ou PRIMARY), demande confirmation si le
/// contenu est multiligne/suspect, puis le transmet via `on_paste`.
fn paste_from(
    primary: bool,
    window_ptr: usize,
    alive: Arc<AtomicBool>,
    locale: &str,
    confirm: bool,
    on_paste: Arc<dyn Fn(String) + Send + Sync>,
) {
    use std::sync::atomic::Ordering;
    let locale = locale.to_string();
    let atom = if primary {
        gtk::gdk::SELECTION_PRIMARY
    } else {
        gtk::gdk::SELECTION_CLIPBOARD
    };
    gtk::Clipboard::get(&atom).request_text(move |_, text| {
        let Some(text) = text.map(|t| t.to_string()) else {
            return;
        };
        if text.is_empty() || !alive.load(Ordering::SeqCst) {
            return;
        }
        if confirm && haraka_core::paste::paste_needs_confirm(&text) {
            let lines = text.matches('\n').count() + text.matches('\r').count() + 1;
            let preview = haraka_core::paste::paste_preview(&text);
            let bytes = text.len();
            confirm_paste(
                window_ptr,
                &alive,
                &locale,
                &preview,
                lines,
                bytes,
                Box::new(move |ok| {
                    if ok {
                        on_paste(text);
                    }
                }),
            );
        } else {
            on_paste(text);
        }
    });
}
