//! Cache keyring OS **optionnel et jamais bloquant** (SECURITY.md §3).
//!
//! La DEK y est déposée pour déverrouiller sans mot de passe maître sur ce
//! poste. Toute erreur (absence de Secret Service, KWallet verrouillé, D-Bus
//! absent) est silencieuse : l'application continue avec le mot de passe maître.
//! Le keyring n'est jamais indispensable au déchiffrement.

#[cfg(target_os = "linux")]
mod imp {
    use keyring::Entry;
    use secrecy::ExposeSecret;

    use crate::crypto::Key256;

    const SERVICE: &str = "harakats";

    fn entry(vault_id: &str) -> Option<Entry> {
        match Entry::new(SERVICE, vault_id) {
            Ok(e) => Some(e),
            Err(err) => {
                tracing::debug!("keyring indisponible (init) : {err}");
                None
            }
        }
    }

    /// Sonde non bloquante : true si un Secret Service répond.
    pub fn available(vault_id: &str) -> bool {
        let Some(e) = entry(vault_id) else {
            return false;
        };
        match e.get_secret() {
            Ok(_) | Err(keyring::Error::NoEntry) => true,
            Err(err) => {
                tracing::debug!("keyring indisponible (sonde) : {err}");
                false
            }
        }
    }

    pub fn store(vault_id: &str, dek: &Key256) -> bool {
        let Some(e) = entry(vault_id) else {
            return false;
        };
        match e.set_secret(dek.expose_secret()) {
            Ok(()) => true,
            Err(err) => {
                tracing::debug!("keyring : dépôt de la DEK impossible : {err}");
                false
            }
        }
    }

    pub fn load(vault_id: &str) -> Option<Vec<u8>> {
        let e = entry(vault_id)?;
        match e.get_secret() {
            Ok(bytes) if bytes.len() == crate::crypto::DEK_LEN => Some(bytes),
            Ok(_) => None,
            Err(keyring::Error::NoEntry) => None,
            Err(err) => {
                tracing::debug!("keyring : lecture impossible : {err}");
                None
            }
        }
    }

    pub fn delete(vault_id: &str) -> bool {
        let Some(e) = entry(vault_id) else {
            return false;
        };
        match e.delete_credential() {
            Ok(()) => true,
            Err(keyring::Error::NoEntry) => true,
            Err(err) => {
                tracing::debug!("keyring : suppression impossible : {err}");
                false
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use crate::crypto::Key256;

    pub fn available(_vault_id: &str) -> bool {
        false
    }

    pub fn store(_vault_id: &str, _dek: &Key256) -> bool {
        false
    }

    pub fn load(_vault_id: &str) -> Option<Vec<u8>> {
        None
    }

    pub fn delete(_vault_id: &str) -> bool {
        false
    }
}

pub use imp::{available, delete, load, store};
