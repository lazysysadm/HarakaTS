//! État applicatif : coffre ouvert (ou non), aperçus d'import en attente,
//! politique d'auto-lock, sessions SSH. La DEK et les secrets ne quittent
//! jamais le backend.

use std::collections::HashMap;
use std::sync::Arc;

use haraka_core::Vault;
use haraka_core::autolock::AutoLock;
use haraka_core::export::ImportPreview;
use haraka_core::store::DataDir;
use haraka_ssh::SessionManager;
use serde::Serialize;
use tokio::sync::Mutex;

pub struct Inner {
    pub vault: Option<Vault>,
    pub previews: HashMap<String, ImportPreview>,
    pub autolock: AutoLock,
    /// Clés privées importées depuis un fichier, en attente d'enregistrement.
    /// Le PEM ne traverse jamais l'IPC : seules les métadonnées sortent.
    pub pending_keys: HashMap<String, PendingKey>,
}

pub struct PendingKey {
    pub pem: secrecy::SecretString,
    pub info: haraka_ssh::KeyInfo,
    pub file_name: String,
}

/// Contexte presse-papier d'une session (M4) : politique effective héritée,
/// limite OSC 52, de quoi répondre à une query et auditer. Capturé à
/// l'ouverture ; partagé par le forwarder de sortie (et le ré-attachement).
#[derive(Clone)]
pub struct ClipCtx {
    pub session_id: String,
    pub sessions: Arc<SessionManager>,
    pub data_dir: haraka_core::store::DataDir,
    pub policy: haraka_core::osc52::ClipboardPolicy,
    pub osc52_max: usize,
    /// Mode « bracketed paste » (DECSET 2004) actif côté terminal : suivi dans
    /// le flux de sortie, utilisé pour encadrer un collage confirmé.
    pub bracketed: Arc<std::sync::atomic::AtomicBool>,
    /// Injection masquée du hook de suivi en cours : nombre de lignes occupées
    /// par l'écho à effacer dès le premier OSC 7 reçu (0 = rien en attente).
    pub hook_rows: Arc<std::sync::atomic::AtomicU32>,
    /// Fin de la sortie récente du shell (séquences d'échappement retirées) : sert à reconnaître le
    /// prompt avant d'injecter le hook de suivi du répertoire.
    pub tail: Arc<std::sync::Mutex<String>>,
    /// Pour émettre `sftp://cwd` (OSC 7) vers la webview.
    pub app: Option<tauri::AppHandle>,
}

/// Contexte par session native (presse-papier, suivi du répertoire).
pub struct SessionIo {
    pub clip: ClipCtx,
    /// Saisie vers la session (consommateur ordonné, encodage de la connexion).
    pub input: InputHandle,
}

/// Envoi de texte comme frappes clavier dans une session native.
#[derive(Clone)]
pub struct InputHandle {
    /// Comme une frappe : diffusée aux autres membres si multi-exec actif.
    pub broadcast: Arc<dyn Fn(String) + Send + Sync>,
    /// Frappe locale uniquement (jamais diffusée) : snippet de démarrage.
    pub local: Arc<dyn Fn(String) + Send + Sync>,
}

/// Entrée du menu « Macros » du terminal natif (jamais le contenu).
#[derive(Clone)]
pub struct MacroEntry {
    pub id: String,
    pub name: String,
    pub confirm: bool,
}

/// État M7 : multi-exec, snippets de démarrage déjà envoyés, cache du menu
/// de macros (lu depuis le thread GTK sans attendre le verrou du coffre).
#[derive(Default)]
pub struct M7State {
    pub multi_exec: haraka_ssh::SharedMultiExec,
    pub startup_done: std::sync::Mutex<std::collections::HashSet<String>>,
    pub macro_menu: std::sync::Mutex<Vec<MacroEntry>>,
}

#[derive(Clone, Serialize)]
pub struct SessionMeta {
    pub connection_id: String,
    pub title: String,
}

pub struct AppState {
    pub inner: Mutex<Inner>,
    pub data_dir: DataDir,
    pub sessions: Arc<SessionManager>,
    /// Moteur de transferts SFTP (M5).
    pub transfers: Arc<haraka_ssh::TransferManager>,
    /// Moteur d'édition à distance (M6).
    pub edits: Arc<haraka_ssh::EditManager>,
    /// Tunnels SSH (M7) et file d'événements consommée par `tunnels::spawn_forwarder`.
    pub tunnels: haraka_ssh::TunnelManager,
    pub tunnel_events:
        std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<haraka_ssh::TunnelEvent>>>,
    /// Renseigné au démarrage (setup) : événements UI émis hors commandes.
    pub app_handle: std::sync::OnceLock<tauri::AppHandle>,
    // Arc : les fenêtres VTE natives capturent ces tables pour leurs
    // callbacks clavier/resize, hors du cycle de vie des commandes IPC.
    pub session_io: Arc<Mutex<HashMap<String, SessionIo>>>,
    pub session_meta: Arc<Mutex<HashMap<String, SessionMeta>>>,
    /// Multi-exec, macros, snippets de démarrage (M7).
    pub m7: M7State,
}

impl AppState {
    pub fn new(
        data_dir: DataDir,
        sessions: Arc<SessionManager>,
        transfers: Arc<haraka_ssh::TransferManager>,
        edits: Arc<haraka_ssh::EditManager>,
    ) -> Self {
        let (tunnels, tunnel_events) = haraka_ssh::TunnelManager::new(sessions.clone());
        Self {
            inner: Mutex::new(Inner {
                vault: None,
                previews: HashMap::new(),
                autolock: AutoLock::new(15),
                pending_keys: HashMap::new(),
            }),
            data_dir,
            sessions,
            transfers,
            edits,
            tunnels,
            tunnel_events: std::sync::Mutex::new(Some(tunnel_events)),
            app_handle: std::sync::OnceLock::new(),
            session_io: Arc::new(Mutex::new(HashMap::new())),
            session_meta: Arc::new(Mutex::new(HashMap::new())),
            m7: M7State::default(),
        }
    }
}
