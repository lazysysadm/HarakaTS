//! Modèle de données du store (ARCHITECTURE.md §4). Tout vit dans le payload
//! chiffré du coffre ; les champs secrets sont des `EncSecret` (AEAD par champ).

use serde::{Deserialize, Serialize};

use crate::crypto::now_rfc3339;
use crate::error::{Error, Result};
use crate::format::PAYLOAD_ALG;

pub const SCHEMA_VERSION: u32 = 1;

/// Secret chiffré individuellement (couche 2). L'AAD est implicite :
/// `vault_id ∥ entry_id ∥ field_name` (voir crypto::aad_secret).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncSecret {
    pub alg: String,
    pub nonce: String,
    pub ct: String,
}

impl EncSecret {
    pub fn new(nonce_b64: String, ct_b64: String) -> Self {
        Self {
            alg: PAYLOAD_ALG.to_string(),
            nonce: nonce_b64,
            ct: ct_b64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReconnectSettings {
    pub enabled: bool,
    pub max_attempts: u32,
    pub backoff_max_s: u32,
}

impl Default for ReconnectSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 5,
            backoff_max_s: 30,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub autolock_minutes: u32,
    pub lock_on_suspend: bool,
    pub lock_on_session_lock: bool,
    pub disconnect_on_lock: bool,
    pub keyring_cache: bool,
    pub clipboard_default: String,
    pub osc52_max_bytes: usize,
    pub paste_confirm_multiline: bool,
    pub copy_on_select: bool,
    pub clipboard_clear_seconds: u32,
    pub locale: String,
    pub theme: String,
    /// Police du terminal natif VTE (appliquée à l'ouverture des fenêtres).
    pub terminal_font_size: u32,
    /// Scrollback du terminal natif VTE (lignes).
    pub terminal_scrollback: u32,
    /// Terminaux natifs en onglets dans la fenêtre principale (au lieu d'une fenêtre par session).
    pub terminal_in_tabs: bool,
    /// Coloration de la sortie (erreurs, avertissements, IP…), règles fixes.
    pub terminal_highlight: bool,
    /// Pastille verte/rouge de joignabilité (connexion TCP vers hôte:port) dans l'arbre.
    pub server_status: bool,
    /// Intervalle entre deux cycles de test, en secondes (minimum 15).
    pub server_status_interval_s: u32,
    pub external_editor: Option<String>,
    pub transfer_parallelism: u32,
    /// Injecte automatiquement, à l'ouverture d'une session native, la ligne qui
    /// active le suivi du répertoire (OSC 7) dans le shell distant (opt-in).
    pub sftp_follow_terminal: bool,
    /// Journaux de session chiffrés avec une sous-clé dérivée de la DEK (`.log.enc`).
    pub session_log_encrypt: bool,
    /// Suppression automatique des journaux plus vieux que N jours (0 = jamais).
    pub session_log_retention_days: u32,
    pub reconnect: ReconnectSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            autolock_minutes: 15,
            lock_on_suspend: true,
            lock_on_session_lock: true,
            disconnect_on_lock: true,
            keyring_cache: false,
            clipboard_default: "bidirectional".to_string(),
            osc52_max_bytes: 100_000,
            paste_confirm_multiline: true,
            copy_on_select: false,
            clipboard_clear_seconds: 30,
            locale: "fr".to_string(),
            theme: "system".to_string(),
            terminal_font_size: 11,
            terminal_scrollback: 10_000,
            terminal_in_tabs: false,
            terminal_highlight: true,
            server_status: false,
            server_status_interval_s: 60,
            external_editor: None,
            transfer_parallelism: 3,
            sftp_follow_terminal: false,
            session_log_encrypt: true,
            session_log_retention_days: 30,
            reconnect: ReconnectSettings::default(),
        }
    }
}

/// Valeurs par défaut héritables d'un dossier (null/absent = pas de défaut).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionDefaults {
    pub user: Option<String>,
    pub port: Option<u16>,
    pub credential_id: Option<String>,
    pub jump_chain: Vec<JumpHop>,
    pub clipboard_policy: Option<String>,
    pub keepalive_s: Option<u32>,
    pub encoding: Option<String>,
    pub auth_method_order: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub sort_order: i32,
    pub tags: Vec<String>,
    pub defaults: ConnectionDefaults,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JumpHop {
    pub host: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub credential_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelSpec {
    pub id: String,
    pub kind: String,
    pub bind_addr: String,
    pub bind_port: u16,
    pub dest_host: Option<String>,
    pub dest_port: Option<u16>,
    pub auto_start: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SftpOptions {
    pub follow_terminal_cwd: bool,
    pub show_hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub folder_id: String,
    pub name: String,
    pub protocol: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub credential_id: Option<String>,
    pub jump_chain: Option<Vec<JumpHop>>,
    pub clipboard_policy: Option<String>,
    pub keepalive_s: Option<u32>,
    pub encoding: Option<String>,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub tags: Vec<String>,
    pub favorite: bool,
    pub tunnels: Vec<TunnelSpec>,
    pub startup_snippet_id: Option<String>,
    pub session_logging: bool,
    pub sftp: SftpOptions,
    pub notes: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub id: String,
    pub name: String,
    pub username: String,
    pub password: Option<EncSecret>,
    pub private_key: Option<EncSecret>,
    pub passphrase: Option<EncSecret>,
    pub key_origin: Option<String>,
    pub needs_reentry: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Macro {
    pub id: String,
    pub name: String,
    pub content: EncSecret,
    pub confirm_before_send: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recent {
    pub connection_id: String,
    pub last_used_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub schema_version: u32,
    pub vault_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub settings: Settings,
    pub folders: Vec<Folder>,
    pub connections: Vec<Connection>,
    pub credentials: Vec<Credential>,
    pub macros: Vec<Macro>,
    pub recents: Vec<Recent>,
    pub ui_state: serde_json::Value,
}

impl Default for Store {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl Store {
    pub fn new(vault_id: String) -> Self {
        let now = now_rfc3339();
        Self {
            schema_version: SCHEMA_VERSION,
            vault_id,
            created_at: now.clone(),
            updated_at: now,
            settings: Settings::default(),
            folders: Vec::new(),
            connections: Vec::new(),
            credentials: Vec::new(),
            macros: Vec::new(),
            recents: Vec::new(),
            ui_state: serde_json::json!({}),
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = now_rfc3339();
    }

    pub fn credential(&self, id: &str) -> Result<&Credential> {
        self.credentials
            .iter()
            .find(|c| c.id == id)
            .ok_or(Error::NotFound("credential"))
    }

    pub fn credential_mut(&mut self, id: &str) -> Result<&mut Credential> {
        self.credentials
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or(Error::NotFound("credential"))
    }

    pub fn folder(&self, id: &str) -> Result<&Folder> {
        self.folders
            .iter()
            .find(|f| f.id == id)
            .ok_or(Error::NotFound("dossier"))
    }

    pub fn macro_by_id(&self, id: &str) -> Result<&Macro> {
        self.macros
            .iter()
            .find(|m| m.id == id)
            .ok_or(Error::NotFound("macro"))
    }

    /// Champs secrets présents d'une entrée (pour vérification itérative).
    pub fn secret_fields(&self, entry_id: &str) -> Vec<crate::vault::SecretField> {
        use crate::vault::SecretField;
        if let Ok(c) = self.credential(entry_id) {
            let mut fields = Vec::new();
            if c.password.is_some() {
                fields.push(SecretField::Password);
            }
            if c.private_key.is_some() {
                fields.push(SecretField::PrivateKey);
            }
            if c.passphrase.is_some() {
                fields.push(SecretField::Passphrase);
            }
            fields
        } else if self.macro_by_id(entry_id).is_ok() {
            vec![SecretField::MacroContent]
        } else {
            Vec::new()
        }
    }
}

/// Migration de schéma : v1 → v1 (identité). Les futures versions ajouteront
/// leurs transformations ici ; une version inconnue est rejetée proprement.
pub fn migrate(store: Store) -> Result<Store> {
    match store.schema_version {
        SCHEMA_VERSION => Ok(store),
        v => Err(Error::UnsupportedSchema(v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_v1_identity_and_unknown_rejected() {
        let store = Store::new("v".into());
        assert!(migrate(store.clone()).is_ok());
        let mut future = store;
        future.schema_version = 99;
        assert!(matches!(migrate(future), Err(Error::UnsupportedSchema(99))));
    }
}
