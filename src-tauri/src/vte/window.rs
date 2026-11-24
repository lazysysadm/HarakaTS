//! Construction de la fenêtre terminal : terminal, barre d'état, câblage des
//! entrées (clavier, presse-papier, taille) et fermeture. Chaque aspect est
//! délégué à son module ; ici on assemble seulement.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gtk::glib;
use gtk::prelude::*;
use tokio::sync::oneshot;

use super::ffi::{Style, Term};
use super::handle::{VteWindow, unregister};
use super::{VteCallbacks, VteOptions, clipboard, resize, tabs};

/// Crée la fenêtre (thread principal GTK). Les premières dimensions réelles
/// sont envoyées sur `first_dims` dès la première allocation.
pub fn create(
    session_id: &str,
    title: &str,
    cb: VteCallbacks,
    first_dims: oneshot::Sender<(u32, u32)>,
    opts: &VteOptions,
) -> Result<VteWindow, String> {
    let alive = Arc::new(AtomicBool::new(true));
    let term = Term::new().ok_or("vte_terminal_new a échoué")?;
    term.apply_style(&Style::default_theme(opts.font_size, opts.scrollback))?;
    let widget = term.widget();

    let (vbox, status) = build_layout(
        &widget,
        &opts.locale,
        cb.on_reconnect.clone(),
        cb.on_sftp.clone(),
    );
    // Fenêtre indépendante, ou onglet de la fenêtre principale (option).
    let (window, window_ptr, page_ptr) = match opts.main_window_ptr {
        Some(main) => {
            let nb = tabs::ensure(main)?;
            let page: gtk::Widget = vbox.clone().upcast();
            let page_ptr = page.as_ptr() as usize;
            let (sid, flag, on_close) =
                (session_id.to_string(), alive.clone(), cb.on_close.clone());
            page.show_all();
            tabs::add_page(&nb, &page, &widget, title, cb.on_focus.clone(), move || {
                super::handle::close_tab(&sid, &flag, page_ptr, &on_close)
            });
            (None, main, page_ptr)
        }
        None => {
            let window = gtk::Window::new(gtk::WindowType::Toplevel);
            window.set_title(&format!("HarakaTS — {title}"));
            window.set_default_size(1100, 700);
            window.add(&vbox);
            let ptr = window.as_ptr() as usize;
            (Some(window), ptr, 0)
        }
    };

    bind_input(&widget, cb.on_input.clone());
    clipboard::bind(
        &widget,
        term,
        window_ptr,
        &alive,
        clipboard::ClipboardOptions {
            locale: opts.locale.clone(),
            paste_confirm: opts.paste_confirm,
            copy_on_select: opts.copy_on_select,
            macro_entries: cb.macro_entries.clone(),
            on_macro: cb.on_macro.clone(),
        },
        cb.on_paste.clone(),
    );
    resize::bind(&widget, term, status.dims, first_dims, cb.on_resize.clone());
    if let Some(window) = &window {
        bind_close(window, session_id, &alive, cb.on_close.clone());
        let on_focus = cb.on_focus.clone();
        window.connect_focus_in_event(move |_, _| {
            on_focus();
            glib::Propagation::Proceed
        });
        window.show_all();
    }
    widget.grab_focus();

    Ok(VteWindow {
        alive,
        vte_ptr: term.addr(),
        window_ptr,
        status_ptr: status.state.as_ptr() as usize,
        multiexec_ptr: status.multiexec.as_ptr() as usize,
        title_base: title.to_string(),
        session_id: session_id.to_string(),
        page_ptr,
        on_close: cb.on_close.clone(),
        locale: opts.locale.clone(),
    })
}

/// Labels de la barre d'état.
struct StatusBar {
    state: gtk::Label,
    multiexec: gtk::Label,
    dims: gtk::Label,
}

/// Terminal au-dessus d'une barre d'état (état/latence, dimensions, Reconnecter).
fn build_layout(
    terminal: &gtk::Widget,
    locale: &str,
    on_reconnect: Arc<dyn Fn() + Send + Sync>,
    on_sftp: Arc<dyn Fn() + Send + Sync>,
) -> (gtk::Box, StatusBar) {
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.pack_start(terminal, true, true, 0);

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let state = gtk::Label::new(Some(&crate::i18n::tr(locale, "native.state.connecting")));
    state.set_halign(gtk::Align::Start);
    let dims = gtk::Label::new(Some(""));
    // Indicateur multi-exec : créé masqué (`show_all` ne le révèle pas : no_show_all).
    let multiexec = gtk::Label::new(None);
    multiexec.set_no_show_all(true);
    multiexec.hide();
    let reconnect = gtk::Button::with_label(&crate::i18n::tr(locale, "native.btn.reconnect"));
    reconnect.connect_clicked(move |_| on_reconnect());
    let sftp_btn = gtk::Button::with_label(&crate::i18n::tr(locale, "native.btn.sftp"));
    sftp_btn.connect_clicked(move |_| on_sftp());
    bar.pack_start(&state, false, false, 6);
    bar.pack_start(&multiexec, false, false, 6);
    bar.pack_start(&dims, false, false, 6);
    bar.pack_end(&reconnect, false, false, 6);
    bar.pack_end(&sftp_btn, false, false, 0);
    vbox.pack_start(&bar, false, false, 2);

    (
        vbox,
        StatusBar {
            state,
            multiexec,
            dims,
        },
    )
}

/// Clavier → SSH (signal « commit » ; canal ordonné par session côté backend).
fn bind_input(widget: &gtk::Widget, on_input: Arc<dyn Fn(String) + Send + Sync>) {
    widget.connect_local("commit", false, move |values| {
        if let Ok(text) = values[1].get::<String>() {
            on_input(text);
        }
        None
    });
}

/// Fermeture → session SSH fermée + nettoyage du registre.
fn bind_close(
    window: &gtk::Window,
    session_id: &str,
    alive: &Arc<AtomicBool>,
    on_close: Arc<dyn Fn() + Send + Sync>,
) {
    let sid = session_id.to_string();
    let alive = alive.clone();
    window.connect_delete_event(move |_, _| {
        tracing::info!(session = %sid, "fenêtre VTE fermée (delete-event)");
        alive.store(false, Ordering::SeqCst);
        unregister(&sid);
        on_close();
        glib::Propagation::Proceed
    });
}
