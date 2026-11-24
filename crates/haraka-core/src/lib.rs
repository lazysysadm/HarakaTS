//! haraka-core — cœur headless de HarakaTS : coffre chiffré portable, modèle de
//! connexions, export/import, audit. Aucune dépendance Tauri/GUI/réseau : tout
//! se teste en CI sans webview (critère d'acceptation n°1 : export A → import B).

pub mod audit;
pub mod autolock;
pub mod crypto;
pub mod effective;
pub mod error;
pub mod export;
pub mod format;
pub mod highlight;
pub mod import;
pub mod keyring_cache;
pub mod knownhosts;
pub mod macros;
pub mod manage;
pub mod model;
pub mod osc52;
pub mod paste;
pub mod quick;
pub mod sessionlog;
pub mod store;
pub mod vault;

pub use effective::{EffectiveConfig, Provenance, Resolved};
pub use error::{Error, Result};
pub use export::{Conflict, ConflictKind, ExportScope, ImportPreview, Strategy};
pub use import::{ImportPlan, ImportReport, ImportSource, ImportSummary, ImportWarning};
pub use model::Store;
pub use quick::QuickSpec;
pub use store::DataDir;
pub use vault::{CreateOptions, SecretField, Vault};
