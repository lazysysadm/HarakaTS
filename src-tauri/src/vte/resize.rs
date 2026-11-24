//! Dimensions du terminal : ouverture du PTY aux vraies dimensions, puis
//! `window_change` SSH débouncé.
//!
//! La grille VTE n'est garantie à jour qu'APRÈS l'émission de size-allocate
//! (le handler utilisateur s'exécute avant celui de VTE) : la lecture est donc
//! différée en idle, sinon on enverrait au serveur l'ancienne taille et htop
//! ne se redessinerait pas. Première allocation → ouverture du PTY ; suivantes
//! → resize SSH débouncé 100 ms (pas un window_change par pixel pendant un drag).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use tokio::sync::oneshot;

use super::ffi::Term;

const DEBOUNCE: Duration = Duration::from_millis(100);
/// Garde-fou : au-delà, la grille lue est aberrante (allocation transitoire).
const MAX_DIM: u32 = 2000;

pub fn bind(
    widget: &gtk::Widget,
    term: Term,
    dims_label: gtk::Label,
    first_dims: oneshot::Sender<(u32, u32)>,
    on_resize: Arc<dyn Fn(u32, u32) + Send + Sync>,
) {
    let first_dims = Rc::new(RefCell::new(Some(first_dims)));
    let pending = Arc::new(Mutex::new((0u32, 0u32)));
    let generation = Arc::new(AtomicU64::new(0));
    let idle_pending = Arc::new(AtomicBool::new(false));

    widget.connect_size_allocate(move |_, _| {
        // Rafale d'allocations : une seule lecture en idle à la fois.
        if idle_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let idle_pending = idle_pending.clone();
        let first_dims = first_dims.clone();
        let pending = pending.clone();
        let generation = generation.clone();
        let on_resize = on_resize.clone();
        let dims_label = dims_label.clone();
        glib::idle_add_local_once(move || {
            idle_pending.store(false, Ordering::SeqCst);
            let (cols, rows) = term.grid_size();
            if cols < 2 || rows < 2 {
                return;
            }
            if cols > MAX_DIM || rows > MAX_DIM {
                tracing::warn!(cols, rows, "dimensions VTE aberrantes, resize ignoré");
                return;
            }
            dims_label.set_text(&format!("{cols}×{rows}"));
            if let Some(tx) = first_dims.borrow_mut().take() {
                let _ = tx.send((cols, rows));
                return;
            }
            schedule_resize(&pending, &generation, &on_resize, cols, rows);
        });
    });
}

/// Mémorise la taille et n'émet que la dernière après 100 ms de calme.
fn schedule_resize(
    pending: &Arc<Mutex<(u32, u32)>>,
    generation: &Arc<AtomicU64>,
    on_resize: &Arc<dyn Fn(u32, u32) + Send + Sync>,
    cols: u32,
    rows: u32,
) {
    *pending.lock().unwrap() = (cols, rows);
    let my_gen = generation.fetch_add(1, Ordering::SeqCst) + 1;
    let generation = generation.clone();
    let pending = pending.clone();
    let on_resize = on_resize.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(DEBOUNCE).await;
        if generation.load(Ordering::SeqCst) != my_gen {
            return;
        }
        let (c, r) = *pending.lock().unwrap();
        tracing::debug!(cols = c, rows = r, "resize VTE → SSH");
        on_resize(c, r);
    });
}
