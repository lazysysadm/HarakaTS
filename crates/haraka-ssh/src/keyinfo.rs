//! Inspection non secrète de clés privées (import depuis un fichier).
//! Ne renvoie que des métadonnées : algorithme, empreinte, commentaire,
//! chiffrement. Le contenu PEM ne quitte jamais le backend.

use russh::keys::{Algorithm, HashAlg, PrivateKey, decode_secret_key};

#[derive(Clone, Debug, serde::Serialize)]
pub struct KeyInfo {
    pub algorithm: String,
    pub fingerprint: String,
    pub comment: String,
    pub encrypted: bool,
}

fn describe(key: &PrivateKey, encrypted: bool) -> KeyInfo {
    let algorithm = match key.algorithm() {
        Algorithm::Ed25519 => "ed25519".to_string(),
        Algorithm::Rsa { .. } => "rsa".to_string(),
        Algorithm::Ecdsa { curve } => format!("ecdsa-{curve:?}"),
        other => format!("{other:?}"),
    };
    KeyInfo {
        algorithm,
        fingerprint: key.public_key().fingerprint(HashAlg::Sha256).to_string(),
        comment: key.comment().to_string(),
        encrypted,
    }
}

/// Inspecte une clé PEM/OpenSSH. Une clé chiffrée est détectée sans
/// passphrase (métadonnées limitées : le contenu privé reste illisible).
pub fn inspect_private_key(pem: &str) -> Result<KeyInfo, String> {
    match decode_secret_key(pem, None) {
        Ok(key) => Ok(describe(&key, false)),
        Err(russh::keys::Error::KeyIsEncrypted) => Ok(KeyInfo {
            algorithm: "chiffrée".to_string(),
            fingerprint: String::new(),
            comment: String::new(),
            encrypted: true,
        }),
        Err(e) => Err(format!("clé illisible : {e}")),
    }
}

/// Vérifie qu'une passphrase déchiffre bien la clé (avant enregistrement).
pub fn check_key_passphrase(pem: &str, passphrase: &str) -> Result<KeyInfo, String> {
    let key = decode_secret_key(pem, Some(passphrase))
        .map_err(|_| "passphrase incorrecte (ou clé illisible)".to_string())?;
    Ok(describe(&key, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::PrivateKey;

    fn gen_ed25519() -> PrivateKey {
        PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
    }

    #[test]
    fn inspect_clear_key() {
        let key = gen_ed25519();
        let pem = key
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap();
        let info = inspect_private_key(&pem).unwrap();
        assert_eq!(info.algorithm, "ed25519");
        assert!(info.fingerprint.starts_with("SHA256:"));
        assert!(!info.encrypted);
    }

    #[test]
    fn inspect_encrypted_key_detects_without_passphrase() {
        let key = gen_ed25519();
        let pem = key
            .encrypt(&mut rand::rng(), "secret-pp")
            .unwrap()
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap();
        let info = inspect_private_key(&pem).unwrap();
        assert!(info.encrypted);
        let ok = check_key_passphrase(&pem, "secret-pp").unwrap();
        assert_eq!(ok.algorithm, "ed25519");
        assert!(check_key_passphrase(&pem, "mauvais").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(inspect_private_key("pas une clé").is_err());
        assert!(inspect_private_key("").is_err());
    }

    #[test]
    fn crlf_pem_accepted() {
        let key = gen_ed25519();
        let pem = key
            .to_openssh(russh::keys::ssh_key::LineEnding::CRLF)
            .unwrap();
        let info = inspect_private_key(&pem).unwrap();
        assert_eq!(info.algorithm, "ed25519");
    }
}
