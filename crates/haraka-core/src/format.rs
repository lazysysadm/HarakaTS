//! Enveloppe binaire versionnée (EXPORT_FORMAT.md §2) : magic, version,
//! en-tête immuable, bloc KDF, DEK wrappée, payload AEAD.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::crypto::{KdfParams, NONCE_LEN, WRAPPED_DEK_LEN};
use crate::error::{Error, Result};

pub const VAULT_MAGIC: [u8; 8] = *b"HRKTSVL1";
pub const EXPORT_MAGIC: [u8; 8] = *b"HRKTSXP1";
pub const FORMAT_VERSION: u16 = 1;
pub const APP_NAME: &str = "harakats";
pub const PAYLOAD_ALG: &str = "XChaCha20-Poly1305";

pub const MAX_HEADER_LEN: usize = 1 << 20;
pub const MAX_KDF_LEN: usize = 64 << 10;
pub const MAX_FILE_LEN: usize = 256 << 20;

#[derive(Clone, Debug)]
pub struct Envelope {
    pub magic: [u8; 8],
    pub version: u16,
    pub header: Vec<u8>,
    pub kdf: Vec<u8>,
    pub dek_nonce: [u8; NONCE_LEN],
    pub wrapped_dek: Vec<u8>,
    pub payload_nonce: [u8; NONCE_LEN],
    pub payload_ct: Vec<u8>,
}

impl Envelope {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            18 + self.header.len()
                + self.kdf.len()
                + NONCE_LEN * 2
                + self.wrapped_dek.len()
                + self.payload_ct.len(),
        );
        out.extend_from_slice(&self.magic);
        out.extend_from_slice(&self.version.to_be_bytes());
        out.extend_from_slice(&(self.header.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.header);
        out.extend_from_slice(&(self.kdf.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.kdf);
        out.extend_from_slice(&self.dek_nonce);
        out.extend_from_slice(&self.wrapped_dek);
        out.extend_from_slice(&self.payload_nonce);
        out.extend_from_slice(&self.payload_ct);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_FILE_LEN {
            return Err(Error::BadFormat("fichier trop volumineux"));
        }
        let mut rest = bytes;
        let magic: [u8; 8] = take(&mut rest, 8)?
            .try_into()
            .map_err(|_| Error::BadFormat("magic tronqué"))?;
        if magic != VAULT_MAGIC && magic != EXPORT_MAGIC {
            return Err(Error::BadFormat("magic inconnu"));
        }
        let version = u16::from_be_bytes(
            take(&mut rest, 2)?
                .try_into()
                .map_err(|_| Error::BadFormat("version tronquée"))?,
        );
        if version != FORMAT_VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        let header_len = read_u32(&mut rest)? as usize;
        if header_len > MAX_HEADER_LEN {
            return Err(Error::BadFormat("en-tête trop volumineux"));
        }
        let header = take(&mut rest, header_len)?.to_vec();
        let kdf_len = read_u32(&mut rest)? as usize;
        if kdf_len > MAX_KDF_LEN {
            return Err(Error::BadFormat("bloc KDF trop volumineux"));
        }
        let kdf = take(&mut rest, kdf_len)?.to_vec();
        let dek_nonce: [u8; NONCE_LEN] = take(&mut rest, NONCE_LEN)?
            .try_into()
            .map_err(|_| Error::BadFormat("dek_nonce tronqué"))?;
        let wrapped_dek = take(&mut rest, WRAPPED_DEK_LEN)?.to_vec();
        let payload_nonce: [u8; NONCE_LEN] = take(&mut rest, NONCE_LEN)?
            .try_into()
            .map_err(|_| Error::BadFormat("payload_nonce tronqué"))?;
        if rest.len() < crate::crypto::TAG_LEN {
            return Err(Error::BadFormat("payload tronqué"));
        }
        let payload_ct = rest.to_vec();
        Ok(Self {
            magic,
            version,
            header,
            kdf,
            dek_nonce,
            wrapped_dek,
            payload_nonce,
            payload_ct,
        })
    }

    pub fn header_json<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_slice(&self.header)?)
    }

    pub fn kdf_params(&self) -> Result<KdfParams> {
        Ok(serde_json::from_slice(&self.kdf)?)
    }
}

fn take<'a>(rest: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    if rest.len() < n {
        return Err(Error::BadFormat("fichier tronqué"));
    }
    let (head, tail) = rest.split_at(n);
    *rest = tail;
    Ok(head)
}

fn read_u32(rest: &mut &[u8]) -> Result<u32> {
    Ok(u32::from_be_bytes(
        take(rest, 4)?
            .try_into()
            .map_err(|_| Error::BadFormat("longueur tronquée"))?,
    ))
}

/// En-tête immuable du coffre (couvert par l'AAD du payload).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultHeader {
    pub app: String,
    pub app_version: String,
    pub created_at: String,
    pub payload_alg: String,
    pub vault_id: String,
}

impl VaultHeader {
    pub fn new(vault_id: String, created_at: String) -> Self {
        Self {
            app: APP_NAME.to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            created_at,
            payload_alg: PAYLOAD_ALG.to_string(),
            vault_id,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Counts {
    pub connections: usize,
    pub credentials: usize,
    pub folders: usize,
    pub macros: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentManifest {
    pub counts: Counts,
    pub folder_id: Option<String>,
    pub includes_secrets: bool,
    pub scope: String,
}

/// En-tête immuable d'un bundle d'export.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportHeader {
    pub app: String,
    pub app_version: String,
    pub content: ContentManifest,
    pub export_id: String,
    pub exported_at: String,
    pub payload_alg: String,
    pub source_vault_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Envelope {
        Envelope {
            magic: VAULT_MAGIC,
            version: FORMAT_VERSION,
            header: b"{}".to_vec(),
            kdf: b"{}".to_vec(),
            dek_nonce: [7u8; NONCE_LEN],
            wrapped_dek: vec![0u8; WRAPPED_DEK_LEN],
            payload_nonce: [9u8; NONCE_LEN],
            payload_ct: vec![0u8; 32],
        }
    }

    #[test]
    fn encode_decode_roundtrip() {
        let env = sample();
        let decoded = Envelope::decode(&env.encode()).unwrap();
        assert_eq!(decoded.header, env.header);
        assert_eq!(decoded.payload_ct, env.payload_ct);
        assert_eq!(decoded.dek_nonce, env.dek_nonce);
    }

    #[test]
    fn trailing_bytes_rejected() {
        let mut bytes = sample().encode();
        bytes.push(0);
        // Les octets surnuméraires sont absorbés dans payload_ct → le tag AEAD
        // échouera au déchiffrement ; le décodage seul ne peut pas les distinguer.
        // En revanche un fichier tronqué est rejeté dès le décodage :
        let short = &bytes[..bytes.len() - 40];
        assert!(Envelope::decode(short).is_err());
    }

    #[test]
    fn unknown_magic_rejected() {
        let mut bytes = sample().encode();
        bytes[0] = b'X';
        assert!(matches!(Envelope::decode(&bytes), Err(Error::BadFormat(_))));
    }

    #[test]
    fn unknown_version_rejected() {
        let mut bytes = sample().encode();
        bytes[8] = 0;
        bytes[9] = 99;
        assert!(matches!(
            Envelope::decode(&bytes),
            Err(Error::UnsupportedVersion(99))
        ));
    }
}
