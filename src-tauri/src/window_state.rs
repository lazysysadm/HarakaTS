//! Taille de la fenêtre principale : démarrage en mode fenêtré (jamais
//! maximisé d'office), la dernière taille (et l'état maximisé) est mémorisée à
//! la fermeture et restaurée au lancement suivant.
//!
//! Fichier `window_state.json` dans le dossier de données (non secret). La
//! position n'est pas gérée : Wayland l'ignore, le gestionnaire de fenêtres
//! décide.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use haraka_core::DataDir;
use serde::{Deserialize, Serialize};
use tauri::{LogicalSize, Manager, WebviewWindow, WindowEvent};

/// Bornes de sécurité (un fichier corrompu ne doit pas produire une fenêtre
/// invisible ou démesurée).
const MIN_W: f64 = 940.0;
const MIN_H: f64 = 600.0;
const MAX_DIM: f64 = 10_000.0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WindowState {
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
}

fn file(dd: &DataDir) -> PathBuf {
    dd.root().join("window_state.json")
}

pub fn load(dd: &DataDir) -> Option<WindowState> {
    let text = std::fs::read_to_string(file(dd)).ok()?;
    let mut ws: WindowState = serde_json::from_str(&text).ok()?;
    if !ws.width.is_finite() || !ws.height.is_finite() {
        return None;
    }
    ws.width = ws.width.clamp(MIN_W, MAX_DIM);
    ws.height = ws.height.clamp(MIN_H, MAX_DIM);
    Some(ws)
}

fn save(dd: &DataDir, ws: &WindowState) {
    let Ok(text) = serde_json::to_string(ws) else {
        return;
    };
    // Écriture atomique : temp puis rename (pas de fichier tronqué si crash).
    let path = file(dd);
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Applique la taille mémorisée (ou la taille par défaut de la config), puis
/// affiche la fenêtre (créée invisible pour éviter le clignotement) et installe
/// le suivi de taille. Toujours appelée avec une fenêtre valide ; `show` est
/// exécuté même si la restauration échoue.
pub fn restore_and_track(app: &tauri::App, dd: &DataDir) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let saved = load(dd);
    if let Some(ws) = saved {
        let _ = window.set_size(LogicalSize::new(ws.width, ws.height));
        if ws.maximized {
            let _ = window.maximize();
        }
    }
    let _ = window.show();
    track(window, dd.clone(), saved);
}

fn track(window: WebviewWindow, dd: DataDir, initial: Option<WindowState>) {
    // Dernière taille « normale » (hors maximisé/plein écran) en pixels logiques.
    let current = window
        .inner_size()
        .ok()
        .map(|s| {
            let f = window.scale_factor().unwrap_or(1.0);
            (f64::from(s.width) / f, f64::from(s.height) / f)
        })
        .unwrap_or((1280.0, 800.0));
    let normal = Arc::new(Mutex::new(match initial {
        Some(ws) => (ws.width, ws.height),
        None => current,
    }));
    let win = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(size) => {
            if win.is_maximized().unwrap_or(false) || win.is_fullscreen().unwrap_or(false) {
                return;
            }
            let f = win.scale_factor().unwrap_or(1.0);
            let (w, h) = (f64::from(size.width) / f, f64::from(size.height) / f);
            if w >= MIN_W && h >= MIN_H {
                *normal.lock().unwrap() = (w, h);
            }
        }
        WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => {
            let (width, height) = *normal.lock().unwrap();
            save(
                &dd,
                &WindowState {
                    width,
                    height,
                    maximized: win.is_maximized().unwrap_or(false),
                },
            );
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_clamps_and_rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let dd = DataDir::open(dir.path().to_path_buf()).unwrap();
        assert!(load(&dd).is_none());
        std::fs::write(file(&dd), "pas du json").unwrap();
        assert!(load(&dd).is_none());
        std::fs::write(
            file(&dd),
            r#"{"width": 10.0, "height": 99999.0, "maximized": true}"#,
        )
        .unwrap();
        let ws = load(&dd).unwrap();
        assert_eq!((ws.width, ws.height, ws.maximized), (MIN_W, MAX_DIM, true));
    }

    #[test]
    fn save_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let dd = DataDir::open(dir.path().to_path_buf()).unwrap();
        let ws = WindowState {
            width: 1100.0,
            height: 700.0,
            maximized: false,
        };
        save(&dd, &ws);
        assert_eq!(load(&dd), Some(ws));
    }
}
