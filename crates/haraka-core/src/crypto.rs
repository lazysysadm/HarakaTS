//! Primitives cryptographiques : Argon2id (KDF), XChaCha20-Poly1305 (AEAD),
//! AAD canoniques, aléatoire système. Aucune crypto maison.
//!
//! Alignement des versions (ARCHITECTURE.md §2.4) : `chacha20poly1305 0.11` →
//! `aead 0.6` ; l'aléatoire vient de `rand 0.10` (`SysRng`, successeur d'`OsRng`,
//! même source système). `argon2 0.6` est utilisé en API brute.

use std::time::{Duration, Instant};

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::TryRng;
use rand::rngs::SysRng;
use secrecy::{ExposeSecret, ExposeSecretMut, SecretBox};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::error::{Error, Result};

pub const DEK_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;
pub const SALT_LEN: usize = 16;
pub const WRAPPED_DEK_LEN: usize = DEK_LEN + TAG_LEN;

/// DEK ou KEK : 32 octets, effacés de la mémoire au drop.
pub type Key256 = SecretBox<[u8; DEK_LEN]>;

/// Paramètres Argon2id stockés en clair (mais authentifiés) dans le bloc KDF.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub alg: String,
    pub m_cost: u32,
    pub p_cost: u32,
    pub salt: String,
    pub t_cost: u32,
    pub version: String,
}

impl KdfParams {
    /// Calibration pour la machine courante (cible 0,5–1 s, EXPORT_FORMAT.md).
    pub fn new_calibrated() -> Result<Self> {
        calibrate(500, 1000)
    }

    /// Paramètres rapides, UNIQUEMENT pour les tests et fixtures publiques.
    /// Ne jamais utiliser pour un coffre réel.
    pub fn insecure_test() -> Result<Self> {
        Self::with_params(4096, 1, 1)
    }

    pub fn with_params(m_cost: u32, t_cost: u32, p_cost: u32) -> Result<Self> {
        Ok(Self {
            alg: "argon2id".to_string(),
            m_cost,
            p_cost,
            salt: b64_encode(&random_array::<SALT_LEN>()?),
            t_cost,
            version: "0x13".to_string(),
        })
    }

    fn argon2(&self) -> Result<Argon2<'_>> {
        if self.alg != "argon2id" || self.version != "0x13" {
            return Err(Error::BadFormat("bloc KDF non supporté"));
        }
        let params = Params::new(self.m_cost, self.t_cost, self.p_cost, Some(DEK_LEN))
            .map_err(|_| Error::InvalidParams("paramètres Argon2id hors bornes".into()))?;
        Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    }
}

/// Aléatoire cryptographique système (remplit entièrement le buffer).
pub fn random_bytes(buf: &mut [u8]) -> Result<()> {
    SysRng.try_fill_bytes(buf).map_err(|_| Error::Crypto)
}

pub fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut buf = [0u8; N];
    random_bytes(&mut buf)?;
    Ok(buf)
}

/// KEK = Argon2id(mot de passe, sel, m, t, p) → 32 octets (API brute).
pub fn derive_kek(password: &[u8], kdf: &KdfParams) -> Result<Key256> {
    let salt = b64_decode(&kdf.salt)?;
    let argon = kdf.argon2()?;
    let mut kek = Key256::new(Box::new([0u8; DEK_LEN]));
    argon
        .hash_password_into(password, &salt, kek.expose_secret_mut())
        .map_err(|_| Error::InvalidParams("dérivation Argon2id impossible".into()))?;
    Ok(kek)
}

/// Calibration : m = 128 MiB, p = 4, t ajusté (2→8) pour viser 0,5–1 s ;
/// m réduit par paliers (plancher 64 MiB) si la machine est lente.
pub fn calibrate(target_min_ms: u64, target_max_ms: u64) -> Result<KdfParams> {
    let mut m_cost = 131_072u32;
    let p_cost = 4u32;
    let password = b"harakats-calibration";
    loop {
        let salt = random_array::<SALT_LEN>()?;
        let probe = KdfParams {
            alg: "argon2id".into(),
            m_cost,
            p_cost,
            salt: b64_encode(&salt),
            t_cost: 2,
            version: "0x13".into(),
        };
        let dur = measure(&probe, password, &salt)?;
        let dur_ms = dur.as_millis() as u64;
        if dur_ms > target_max_ms && m_cost > 65_536 {
            m_cost /= 2;
            continue;
        }
        let target_ns = ((target_min_ms + target_max_ms) as f64 / 2.0) * 1_000_000.0;
        let t_cost = if dur.as_nanos() == 0 {
            8
        } else {
            ((target_ns / dur.as_nanos() as f64) * 2.0).round() as u32
        };
        return Ok(KdfParams {
            t_cost: t_cost.clamp(2, 8),
            ..probe
        });
    }
}

fn measure(kdf: &KdfParams, password: &[u8], salt: &[u8]) -> Result<Duration> {
    let argon = kdf.argon2()?;
    let mut out = [0u8; DEK_LEN];
    let start = Instant::now();
    argon
        .hash_password_into(password, salt, &mut out)
        .map_err(|_| Error::InvalidParams("dérivation Argon2id impossible".into()))?;
    let elapsed = start.elapsed();
    out.zeroize();
    Ok(elapsed)
}

/// AEAD avec AAD. `nonce` doit faire exactement NONCE_LEN octets.
pub fn seal(
    key: &[u8; DEK_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| Error::Crypto)?;
    cipher
        .encrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| Error::Crypto)
}

pub fn open(
    key: &[u8; DEK_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| Error::Crypto)?;
    cipher
        .decrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| Error::Tampered)
}

/// AAD du payload : lie le ciphertext à l'en-tête immuable (EXPORT_FORMAT.md §2).
pub fn aad_payload(magic: &[u8; 8], version: u16, header: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(14 + header.len());
    v.extend_from_slice(magic);
    v.extend_from_slice(&version.to_be_bytes());
    v.extend_from_slice(&(header.len() as u32).to_be_bytes());
    v.extend_from_slice(header);
    v
}

/// AAD du wrap de la DEK : lie la DEK wrappée à l'identité du conteneur et au
/// bloc KDF exact (sel et paramètres authentifiés par le tag du wrap).
pub fn aad_wrapped_dek(container_id: &str, kdf_bytes: &[u8]) -> Vec<u8> {
    let mut v = b"HRKTS/DEK\0".to_vec();
    v.extend_from_slice(container_id.as_bytes());
    v.extend_from_slice(&(kdf_bytes.len() as u32).to_be_bytes());
    v.extend_from_slice(kdf_bytes);
    v
}

/// AAD par secret : vault_id/export_id ∥ entry_id ∥ nom du champ, avec préfixes
/// de longueur u32be (anti-permutation, anti-ambiguïté de concaténation).
pub fn aad_secret(container_id: &str, entry_id: &str, field: &str) -> Vec<u8> {
    let mut v = Vec::new();
    for part in [container_id, entry_id, field] {
        v.extend_from_slice(&(part.len() as u32).to_be_bytes());
        v.extend_from_slice(part.as_bytes());
    }
    v
}

pub fn wrap_dek(
    kek: &Key256,
    dek: &Key256,
    container_id: &str,
    kdf_bytes: &[u8],
) -> Result<([u8; NONCE_LEN], Vec<u8>)> {
    let nonce = random_array::<NONCE_LEN>()?;
    let ct = seal(
        kek.expose_secret(),
        &nonce,
        &aad_wrapped_dek(container_id, kdf_bytes),
        dek.expose_secret(),
    )?;
    debug_assert_eq!(ct.len(), WRAPPED_DEK_LEN);
    Ok((nonce, ct))
}

pub fn unwrap_dek(
    kek: &Key256,
    nonce: &[u8; NONCE_LEN],
    wrapped: &[u8],
    container_id: &str,
    kdf_bytes: &[u8],
) -> Result<Key256> {
    let mut pt = open(
        kek.expose_secret(),
        nonce,
        &aad_wrapped_dek(container_id, kdf_bytes),
        wrapped,
    )?;
    if pt.len() != DEK_LEN {
        pt.zeroize();
        return Err(Error::Tampered);
    }
    // Copie directement dans la boîte du tas (`init_with_mut`) : aucun tableau intermédiaire non effacé sur la pile.
    let dek = Key256::init_with_mut(|d| d.copy_from_slice(&pt));
    pt.zeroize();
    Ok(dek)
}

/// JSON canonique : clés triées (BTreeMap par défaut de serde_json), entiers
/// sans exposant, aucun espace superflu. Les octets produits font l'AAD.
pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let v = serde_json::to_value(value)?;
    Ok(serde_json::to_vec(&v)?)
}

pub fn b64_encode(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

pub fn b64_decode(text: &str) -> Result<Vec<u8>> {
    B64.decode(text)
        .map_err(|_| Error::BadFormat("base64 invalide"))
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrip_with_aad() {
        let key = random_array::<DEK_LEN>().unwrap();
        let nonce = random_array::<NONCE_LEN>().unwrap();
        let ct = seal(&key, &nonce, b"aad", b"secret").unwrap();
        let pt = open(&key, &nonce, b"aad", &ct).unwrap();
        assert_eq!(pt, b"secret");
    }

    #[test]
    fn wrong_aad_rejected() {
        let key = random_array::<DEK_LEN>().unwrap();
        let nonce = random_array::<NONCE_LEN>().unwrap();
        let ct = seal(&key, &nonce, b"aad-1", b"secret").unwrap();
        assert!(matches!(
            open(&key, &nonce, b"aad-2", &ct),
            Err(Error::Tampered)
        ));
    }

    #[test]
    fn aad_secret_length_prefixes_prevent_ambiguity() {
        assert_ne!(aad_secret("ab", "c", "d"), aad_secret("a", "bc", "d"));
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let v: serde_json::Value = serde_json::from_str(r#"{"b":1,"a":2}"#).unwrap();
        assert_eq!(canonical_json(&v).unwrap(), br#"{"a":2,"b":1}"#);
    }

    #[test]
    fn kdf_derivation_deterministic_and_salt_sensitive() {
        let kdf = KdfParams::insecure_test().unwrap();
        let k1 = derive_kek(b"pw", &kdf).unwrap();
        let k2 = derive_kek(b"pw", &kdf).unwrap();
        assert_eq!(k1.expose_secret(), k2.expose_secret());
        let other = KdfParams::insecure_test().unwrap();
        let k3 = derive_kek(b"pw", &other).unwrap();
        assert_ne!(k1.expose_secret(), k3.expose_secret());
    }

    #[test]
    fn calibration_produces_valid_params() {
        let kdf = calibrate(5, 50).unwrap();
        assert_eq!(kdf.alg, "argon2id");
        assert!(kdf.m_cost >= 65_536);
        assert!((2..=8).contains(&kdf.t_cost));
        assert_eq!(kdf.p_cost, 4);
        assert!(derive_kek(b"pw", &kdf).is_ok());
    }
}
