//! FFI libvte-2.91 / pango et wrapper sûr `Term`.
//!
//! Toute la surface `unsafe` du terminal natif est ICI. Le reste du module
//! manipule `Term` (méthodes sûres) ou des adresses `usize` quand un pointeur
//! doit traverser un thread (jamais déréférencées hors du thread GTK).
//!
//! Invariants (documentés une fois, vérifiés par les appelants) :
//! - un `Term` n'est utilisé que sur le thread principal GTK ;
//! - le widget reste valide tant que la fenêtre vit : les appelants cross-thread
//!   testent `alive` (voir `handle::on_main_if_alive`) AVANT de reconstruire un
//!   `Term` depuis son adresse.

use std::ffi::{CString, c_char, c_int, c_long, c_void};

use gtk::glib::translate::FromGlibPtrNone;

/// GdkRGBA (ABI C stable) — évite de dépendre des types gdk-rs internes.
#[repr(C)]
struct GdkRGBA {
    red: f64,
    green: f64,
    blue: f64,
    alpha: f64,
}

#[link(name = "vte-2.91")]
unsafe extern "C" {
    fn vte_terminal_new() -> *mut c_void;
    fn vte_terminal_feed(term: *mut c_void, data: *const c_char, length: isize);
    fn vte_terminal_get_column_count(term: *mut c_void) -> c_long;
    fn vte_terminal_get_row_count(term: *mut c_void) -> c_long;
    fn vte_terminal_get_cursor_position(term: *mut c_void, column: *mut c_long, row: *mut c_long);
    fn vte_terminal_set_scrollback_lines(term: *mut c_void, lines: c_long);
    fn vte_terminal_set_font(term: *mut c_void, desc: *const c_void);
    fn vte_terminal_set_colors(
        term: *mut c_void,
        foreground: *const GdkRGBA,
        background: *const GdkRGBA,
        palette: *const GdkRGBA,
        palette_size: usize,
    );
    fn vte_terminal_set_cursor_blink_mode(term: *mut c_void, mode: c_int);
    fn vte_terminal_set_mouse_autohide(term: *mut c_void, setting: c_int);
    fn vte_terminal_copy_clipboard_format(term: *mut c_void, format: c_int);
    fn vte_terminal_select_all(term: *mut c_void);
    fn vte_terminal_reset(term: *mut c_void, clear_scrollback: c_int);
    fn vte_terminal_get_has_selection(term: *mut c_void) -> c_int;
}

#[link(name = "pango-1.0")]
unsafe extern "C" {
    fn pango_font_description_from_string(text: *const c_char) -> *mut c_void;
    fn pango_font_description_free(desc: *mut c_void);
}

/// Format de copie VTE : texte brut (`VTE_FORMAT_TEXT`).
const VTE_FORMAT_TEXT: c_int = 1;
/// Clignotement du curseur forcé (`VTE_CURSOR_BLINK_ON`).
const VTE_CURSOR_BLINK_ON: c_int = 1;

/// Widget `VteTerminal` (thread principal GTK uniquement, cf. invariants).
#[derive(Clone, Copy)]
pub struct Term(*mut c_void);

impl Term {
    /// Crée un terminal VTE (None si l'allocation échoue).
    pub fn new() -> Option<Self> {
        let ptr = unsafe { vte_terminal_new() };
        (!ptr.is_null()).then_some(Self(ptr))
    }

    /// Reconstruit un `Term` depuis son adresse.
    ///
    /// # Safety
    /// L'appelant est sur le thread GTK et la fenêtre propriétaire est vivante.
    pub unsafe fn from_addr(addr: usize) -> Self {
        Self(addr as *mut c_void)
    }

    pub fn addr(self) -> usize {
        self.0 as usize
    }

    /// Le widget GTK correspondant (référence comptée).
    pub fn widget(self) -> gtk::Widget {
        unsafe { gtk::Widget::from_glib_none(self.0 as *mut gtk::ffi::GtkWidget) }
    }

    /// Police (« Monospace 11 »), couleurs, scrollback, curseur.
    pub fn apply_style(self, style: &Style) -> Result<(), String> {
        let font_desc = CString::new(style.font.as_str()).map_err(|_| "police invalide")?;
        let fg = GdkRGBA {
            red: style.foreground[0],
            green: style.foreground[1],
            blue: style.foreground[2],
            alpha: 1.0,
        };
        let bg = GdkRGBA {
            red: style.background[0],
            green: style.background[1],
            blue: style.background[2],
            alpha: 1.0,
        };
        unsafe {
            let font = pango_font_description_from_string(font_desc.as_ptr());
            if !font.is_null() {
                vte_terminal_set_font(self.0, font);
                pango_font_description_free(font);
            }
            vte_terminal_set_colors(self.0, &fg, &bg, std::ptr::null(), 0);
            vte_terminal_set_scrollback_lines(self.0, c_long::from(style.scrollback));
            vte_terminal_set_cursor_blink_mode(self.0, VTE_CURSOR_BLINK_ON);
            vte_terminal_set_mouse_autohide(self.0, 1);
        }
        Ok(())
    }

    /// Injecte du texte UTF-8 (sortie serveur) dans l'émulateur.
    pub fn feed(self, text: &str) {
        unsafe {
            vte_terminal_feed(self.0, text.as_ptr() as *const c_char, text.len() as isize);
        }
    }

    /// Dimensions de la grille (colonnes, lignes). Valeurs à jour seulement
    /// APRÈS le size-allocate interne de VTE (voir `resize`).
    pub fn grid_size(self) -> (u32, u32) {
        let cols = unsafe { vte_terminal_get_column_count(self.0) };
        let rows = unsafe { vte_terminal_get_row_count(self.0) };
        (cols.max(0) as u32, rows.max(0) as u32)
    }

    /// Position du curseur (colonne, ligne) dans la grille visible.
    pub fn cursor_position(self) -> (u32, u32) {
        let (mut col, mut row): (c_long, c_long) = (0, 0);
        unsafe { vte_terminal_get_cursor_position(self.0, &mut col, &mut row) };
        (col.max(0) as u32, row.max(0) as u32)
    }

    pub fn copy_selection(self) {
        unsafe { vte_terminal_copy_clipboard_format(self.0, VTE_FORMAT_TEXT) }
    }

    pub fn has_selection(self) -> bool {
        unsafe { vte_terminal_get_has_selection(self.0) != 0 }
    }

    pub fn select_all(self) {
        unsafe { vte_terminal_select_all(self.0) }
    }

    pub fn reset(self, clear_scrollback: bool) {
        unsafe { vte_terminal_reset(self.0, c_int::from(clear_scrollback)) }
    }
}

/// Apparence du terminal (couleurs en RGB 0..1).
pub struct Style {
    pub font: String,
    pub foreground: [f64; 3],
    pub background: [f64; 3],
    pub scrollback: u32,
}

impl Style {
    /// Thème par défaut (Catppuccin Mocha) avec police/scrollback réglables.
    pub fn default_theme(font_size: u32, scrollback: u32) -> Self {
        Self {
            font: format!("Monospace {font_size}"),
            foreground: [0.804, 0.839, 0.957],
            background: [0.086, 0.086, 0.118],
            scrollback,
        }
    }
}

/// Reconstruit un `gtk::Window` depuis son adresse.
///
/// # Safety
/// Thread GTK + fenêtre vivante (cf. invariants du module).
pub unsafe fn window_from_addr(addr: usize) -> gtk::Window {
    unsafe { gtk::Window::from_glib_none(addr as *mut gtk::ffi::GtkWindow) }
}

/// Reconstruit un `gtk::Widget` depuis son adresse.
///
/// # Safety
/// Thread GTK + widget vivant (cf. invariants du module).
pub unsafe fn widget_from_addr(addr: usize) -> gtk::Widget {
    unsafe { gtk::Widget::from_glib_none(addr as *mut gtk::ffi::GtkWidget) }
}

/// Reconstruit un `gtk::Label` depuis son adresse.
///
/// # Safety
/// Thread GTK + fenêtre vivante (cf. invariants du module).
pub unsafe fn label_from_addr(addr: usize) -> gtk::Label {
    unsafe { gtk::Label::from_glib_none(addr as *mut gtk::ffi::GtkLabel) }
}
