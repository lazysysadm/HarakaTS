//! Dialogues GTK modaux de la fenêtre terminal : authentification
//! (keyboard-interactive), clé d'hôte (TOFU / changement) et confirmation de
//! collage. Un seul chemin de construction (`modal`) et de réponse (`on_response`).

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gtk::glib;
use gtk::prelude::*;
use haraka_ssh::PromptDto;

use super::ffi::window_from_addr;
use super::handle::VteWindow;

/// Dialogue modal rattaché à la fenêtre terminal.
fn modal(parent_ptr: usize, title: &str) -> gtk::Dialog {
    let parent = unsafe { window_from_addr(parent_ptr) };
    let dialog = gtk::Dialog::new();
    dialog.set_title(title);
    dialog.set_transient_for(Some(&parent));
    dialog.set_modal(true);
    dialog
}

/// Branche la réponse : `respond` est appelé UNE fois (même si le signal est
/// émis plusieurs fois), puis le dialogue est toujours détruit.
fn on_response<F>(dialog: &gtk::Dialog, respond: F)
where
    F: FnOnce(gtk::ResponseType) + 'static,
{
    let respond = RefCell::new(Some(respond));
    dialog.connect_response(move |d, resp| {
        if let Some(f) = respond.borrow_mut().take() {
            f(resp);
        }
        unsafe { d.destroy() };
    });
}

fn wrapped_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_line_wrap(true);
    label.set_halign(gtk::Align::Start);
    label
}

/// Dialogue GTK natif pour keyboard-interactive. Annuler envoie une réponse
/// vide : le serveur rejette proprement au lieu d'attendre son timeout.
pub fn show_auth_prompt(
    win: VteWindow,
    name: String,
    instructions: String,
    prompts: Vec<PromptDto>,
    respond: Box<dyn FnOnce(Vec<String>) + Send>,
) {
    glib::MainContext::default().invoke(move || {
        if !win.alive.load(Ordering::SeqCst) {
            return;
        }
        let l = |key: &str| crate::i18n::tr(&win.locale, key);
        let title = if name.is_empty() {
            l("native.dialog.auth")
        } else {
            name.clone()
        };
        let dialog = modal(win.window_ptr, &title);
        let area = dialog.content_area();
        if !instructions.is_empty() {
            area.pack_start(&wrapped_label(&instructions), false, false, 4);
        }
        let mut entries = Vec::new();
        for prompt in &prompts {
            let label = gtk::Label::new(Some(&prompt.text));
            label.set_halign(gtk::Align::Start);
            area.pack_start(&label, false, false, 2);
            let entry = gtk::Entry::new();
            entry.set_visibility(prompt.echo);
            area.pack_start(&entry, false, false, 2);
            entries.push(entry);
        }
        dialog.add_button(&l("native.btn.cancel"), gtk::ResponseType::Cancel);
        dialog.add_button(&l("native.btn.send"), gtk::ResponseType::Ok);
        on_response(&dialog, move |resp| {
            let answers = if resp == gtk::ResponseType::Ok {
                entries.iter().map(|e| e.text().to_string()).collect()
            } else {
                Vec::new()
            };
            respond(answers);
        });
        dialog.show_all();
    });
}

/// Dialogue GTK natif pour clé d'hôte : TOFU ou changement (alerte MITM).
pub fn show_hostkey_prompt(
    win: VteWindow,
    host: String,
    port: u16,
    changed: bool,
    fingerprint: String,
    old_fingerprint: Option<String>,
    respond: Box<dyn FnOnce(bool, bool) + Send>,
) {
    glib::MainContext::default().invoke(move || {
        if !win.alive.load(Ordering::SeqCst) {
            return;
        }
        let l = |key: &str| crate::i18n::tr(&win.locale, key);
        let title = if changed {
            l("native.hostkey.changedTitle")
        } else {
            l("native.hostkey.newTitle")
        };
        let dialog = modal(win.window_ptr, &title);
        let port_text = port.to_string();
        let text = if changed {
            let old = old_fingerprint.unwrap_or_else(|| l("native.hostkey.unknownOld"));
            crate::i18n::tr_with(
                &win.locale,
                "native.hostkey.changedText",
                &[
                    ("host", &host),
                    ("port", &port_text),
                    ("old", &old),
                    ("fingerprint", &fingerprint),
                ],
            )
        } else {
            crate::i18n::tr_with(
                &win.locale,
                "native.hostkey.newText",
                &[
                    ("host", &host),
                    ("port", &port_text),
                    ("fingerprint", &fingerprint),
                ],
            )
        };
        dialog
            .content_area()
            .pack_start(&wrapped_label(&text), false, false, 8);
        if changed {
            // Blocage dur : un seul bouton, la connexion est déjà refusée côté session.
            dialog.add_button(&l("native.btn.close"), gtk::ResponseType::Close);
            on_response(&dialog, move |_| respond(false, false));
        } else {
            dialog.add_button(&l("native.btn.refuse"), gtk::ResponseType::Cancel);
            dialog.add_button(&l("native.btn.acceptRemember"), gtk::ResponseType::Ok);
            on_response(&dialog, move |resp| {
                let accept = resp == gtk::ResponseType::Ok;
                respond(accept, accept);
            });
        }
        dialog.show_all();
    });
}

/// Dialogue de confirmation de collage (M4) : aperçu EXACT (contrôles rendus
/// visibles ␛ ␍ ␊), anti-pastejacking. Annuler n'envoie rien ; un seul
/// dialogue par collage, toujours détruit à la réponse.
pub fn confirm_paste(
    window_ptr: usize,
    alive: &Arc<AtomicBool>,
    locale: &str,
    preview: &str,
    lines: usize,
    bytes: usize,
    respond: Box<dyn FnOnce(bool)>,
) {
    let (lines_text, bytes_text) = (lines.to_string(), bytes.to_string());
    confirm_preview(
        window_ptr,
        alive,
        locale,
        ("native.paste.title", "native.btn.paste"),
        &crate::i18n::tr_with(
            locale,
            "native.paste.info",
            &[("lines", &lines_text), ("bytes", &bytes_text)],
        ),
        preview,
        respond,
    );
}

/// Dialogue « Envoyer la macro ? » : même aperçu exact que le collage. Appelable
/// depuis n'importe quel thread ; `respond(true)` seulement si l'utilisateur confirme.
pub fn ask_macro_confirm(
    win: VteWindow,
    name: String,
    preview: String,
    lines: usize,
    bytes: usize,
    respond: Box<dyn FnOnce(bool) + Send>,
) {
    glib::MainContext::default().invoke(move || {
        let (lines_text, bytes_text) = (lines.to_string(), bytes.to_string());
        confirm_preview(
            win.window_ptr,
            &win.alive,
            &win.locale,
            ("native.macro.confirmTitle", "native.macro.send"),
            &crate::i18n::tr_with(
                &win.locale,
                "native.macro.confirmInfo",
                &[
                    ("name", &name),
                    ("lines", &lines_text),
                    ("bytes", &bytes_text),
                ],
            ),
            &preview,
            respond,
        );
    });
}

/// Corps commun : texte d'information, aperçu défilant, Annuler (défaut) / OK.
/// Un seul dialogue, toujours détruit à la réponse.
fn confirm_preview(
    window_ptr: usize,
    alive: &Arc<AtomicBool>,
    locale: &str,
    (title_key, ok_key): (&str, &str),
    info: &str,
    preview: &str,
    respond: Box<dyn FnOnce(bool)>,
) {
    if !alive.load(Ordering::SeqCst) {
        respond(false);
        return;
    }
    let l = |key: &str| crate::i18n::tr(locale, key);
    let dialog = modal(window_ptr, &l(title_key));
    let area = dialog.content_area();
    area.pack_start(&wrapped_label(info), false, false, 6);
    let scrolled = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scrolled.set_size_request(520, 280);
    scrolled.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    let label = gtk::Label::new(None);
    label.set_markup(&format!("<tt>{}</tt>", glib::markup_escape_text(preview)));
    label.set_halign(gtk::Align::Start);
    label.set_valign(gtk::Align::Start);
    label.set_selectable(true);
    scrolled.add(&label);
    area.pack_start(&scrolled, true, true, 6);
    dialog.add_button(&l("native.btn.cancel"), gtk::ResponseType::Cancel);
    dialog.add_button(&l(ok_key), gtk::ResponseType::Ok);
    dialog.set_default_response(gtk::ResponseType::Cancel);
    on_response(&dialog, move |resp| respond(resp == gtk::ResponseType::Ok));
    dialog.show_all();
}
