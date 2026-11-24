//! Le coffre : DEK/KEK, création, déverrouillage (master ou keyring),
//! verrouillage, changement de master (O(1)), accès aux secrets, sauvegarde.

use secrecy::{ExposeSecret, SecretBox, SecretString};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::audit;
use crate::crypto::{
    self, DEK_LEN, KdfParams, Key256, NONCE_LEN, SALT_LEN, aad_payload, aad_secret, b64_decode,
    b64_encode, canonical_json, derive_kek, now_rfc3339, random_array, unwrap_dek, wrap_dek,
};
use crate::error::{Error, Result};
use crate::format::{Envelope, FORMAT_VERSION, VAULT_MAGIC, VaultHeader};
use crate::keyring_cache;
use crate::model::{EncSecret, Store};
use crate::store::{self, DataDir, atomic_write, rotate_backups};

/// Champs secrets individuellement chiffrés (AAD par champ).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretField {
    Password,
    PrivateKey,
    Passphrase,
    MacroContent,
}

impl SecretField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::PrivateKey => "private_key",
            Self::Passphrase => "passphrase",
            Self::MacroContent => "macro_content",
        }
    }

    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "password" => Ok(Self::Password),
            "private_key" => Ok(Self::PrivateKey),
            "passphrase" => Ok(Self::Passphrase),
            "macro_content" => Ok(Self::MacroContent),
            _ => Err(Error::InvalidReference(format!(
                "champ secret inconnu : {name}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CreateOptions {
    /// Cache keyring opt-in (jamais bloquant, désactivé par défaut).
    pub keyring_cache: bool,
    /// Paramètres KDF imposés (tests/fixtures) ; None → calibration 0,5–1 s.
    pub kdf: Option<KdfParams>,
}

pub struct Vault {
    pub store: Store,
    header: VaultHeader,
    kdf: KdfParams,
    dek_nonce: [u8; NONCE_LEN],
    wrapped_dek: Vec<u8>,
    payload_nonce: [u8; NONCE_LEN],
    payload_ct: Vec<u8>,
    dek: Key256,
    data_dir: DataDir,
}

impl Vault {
    /// Crée un coffre neuf. Échoue si un coffre existe déjà.
    pub fn create(dd: DataDir, master: &SecretString, opts: CreateOptions) -> Result<Self> {
        if dd.vault_exists() {
            return Err(Error::AlreadyExists);
        }
        let kdf = match opts.kdf {
            Some(k) => k,
            None => KdfParams::new_calibrated()?,
        };
        let dek: Key256 = SecretBox::new(Box::new(random_array::<DEK_LEN>()?));
        let vault_id = Uuid::new_v4().to_string();
        let header = VaultHeader::new(vault_id.clone(), now_rfc3339());
        let mut store = Store::new(vault_id.clone());
        store.settings.keyring_cache = opts.keyring_cache;

        let header_bytes = canonical_json(&header)?;
        let kdf_bytes = canonical_json(&kdf)?;
        let mut payload = canonical_json(&store)?;
        let payload_nonce = random_array::<NONCE_LEN>()?;
        let payload_ct = crypto::seal(
            dek.expose_secret(),
            &payload_nonce,
            &aad_payload(&VAULT_MAGIC, FORMAT_VERSION, &header_bytes),
            &payload,
        )?;
        payload.zeroize();

        let kek = derive_kek(master.expose_secret().as_bytes(), &kdf)?;
        let (dek_nonce, wrapped_dek) = wrap_dek(&kek, &dek, &vault_id, &kdf_bytes)?;

        let envelope = Envelope {
            magic: VAULT_MAGIC,
            version: FORMAT_VERSION,
            header: header_bytes,
            kdf: kdf_bytes,
            dek_nonce,
            wrapped_dek,
            payload_nonce,
            payload_ct,
        };
        atomic_write(&dd.vault_file(), &envelope.encode())?;
        audit::append(&dd, "vault_create", Some(&vault_id), None)?;
        if opts.keyring_cache {
            keyring_cache::store(&vault_id, &dek);
        }
        Ok(Self {
            store,
            header,
            kdf,
            dek_nonce,
            wrapped_dek: envelope.wrapped_dek,
            payload_nonce,
            payload_ct: envelope.payload_ct,
            dek,
            data_dir: dd,
        })
    }

    /// Déverrouillage par mot de passe maître.
    pub fn unlock(dd: DataDir, master: &SecretString) -> Result<Self> {
        let envelope = read_envelope(&dd)?;
        let header: VaultHeader = envelope.header_json()?;
        let kdf = envelope.kdf_params()?;
        let kek = derive_kek(master.expose_secret().as_bytes(), &kdf)?;
        let dek = unwrap_dek(
            &kek,
            &envelope.dek_nonce,
            &envelope.wrapped_dek,
            &header.vault_id,
            &envelope.kdf,
        )
        .map_err(|_| Error::WrongPassword)?;
        let vault = Self::from_envelope(dd, envelope, header, kdf, dek)?;
        audit::append(
            &vault.data_dir,
            "vault_unlock",
            Some(&vault.store.vault_id),
            Some("master"),
        )?;
        Ok(vault)
    }

    /// Déverrouillage via le cache keyring (optionnel, jamais bloquant).
    /// Échoue proprement si le cache est absent → l'UI retombe sur le master.
    pub fn unlock_with_keyring(dd: DataDir) -> Result<Self> {
        let envelope = read_envelope(&dd)?;
        let header: VaultHeader = envelope.header_json()?;
        let kdf = envelope.kdf_params()?;
        let mut raw =
            keyring_cache::load(&header.vault_id).ok_or(Error::NotFound("cache keyring"))?;
        if raw.len() != DEK_LEN {
            raw.zeroize();
            return Err(Error::Tampered);
        }
        let dek: Key256 = SecretBox::init_with_mut(|d: &mut [u8; DEK_LEN]| d.copy_from_slice(&raw));
        raw.zeroize();
        let vault = Self::from_envelope(dd, envelope, header, kdf, dek)?;
        audit::append(
            &vault.data_dir,
            "vault_unlock",
            Some(&vault.store.vault_id),
            Some("keyring"),
        )?;
        Ok(vault)
    }

    fn from_envelope(
        dd: DataDir,
        envelope: Envelope,
        header: VaultHeader,
        kdf: KdfParams,
        dek: Key256,
    ) -> Result<Self> {
        if envelope.magic != VAULT_MAGIC {
            return Err(Error::BadFormat("magic de coffre attendu"));
        }
        let mut payload = crypto::open(
            dek.expose_secret(),
            &envelope.payload_nonce,
            &aad_payload(&envelope.magic, envelope.version, &envelope.header),
            &envelope.payload_ct,
        )?;
        let store: Store = serde_json::from_slice(&payload)?;
        payload.zeroize();
        let store = crate::model::migrate(store)?;
        if store.vault_id != header.vault_id {
            return Err(Error::Tampered);
        }
        verify_secrets(&store, &dek, &header.vault_id)?;
        Ok(Self {
            store,
            header,
            kdf,
            dek_nonce: envelope.dek_nonce,
            wrapped_dek: envelope.wrapped_dek,
            payload_nonce: envelope.payload_nonce,
            payload_ct: envelope.payload_ct,
            dek,
            data_dir: dd,
        })
    }

    /// Verrouillage : audit puis effacement de la DEK (zeroize au drop).
    pub fn lock(self) {
        let _ = audit::append(
            &self.data_dir,
            "vault_lock",
            Some(&self.store.vault_id),
            None,
        );
        drop(self);
    }

    /// Changement de mot de passe maître : re-wrap de la DEK uniquement.
    /// L'en-tête immuable et le payload ne sont pas modifiés (O(1)).
    pub fn change_master(&mut self, old: &SecretString, new: &SecretString) -> Result<()> {
        let kdf_bytes = canonical_json(&self.kdf)?;
        let kek_old = derive_kek(old.expose_secret().as_bytes(), &self.kdf)?;
        let _ = unwrap_dek(
            &kek_old,
            &self.dek_nonce,
            &self.wrapped_dek,
            &self.header.vault_id,
            &kdf_bytes,
        )
        .map_err(|_| Error::WrongPassword)?;

        let mut kdf = self.kdf.clone();
        kdf.salt = b64_encode(&random_array::<SALT_LEN>()?);
        let new_kdf_bytes = canonical_json(&kdf)?;
        let kek_new = derive_kek(new.expose_secret().as_bytes(), &kdf)?;
        let (nonce, wrapped) =
            wrap_dek(&kek_new, &self.dek, &self.header.vault_id, &new_kdf_bytes)?;
        self.kdf = kdf;
        self.dek_nonce = nonce;
        self.wrapped_dek = wrapped;
        self.write_envelope()?;
        audit::append(
            &self.data_dir,
            "master_change",
            Some(&self.store.vault_id),
            None,
        )?;
        Ok(())
    }

    /// Sérialise le store, re-scelle le payload (nouveau nonce) et écrit
    /// atomiquement avec rotation des sauvegardes.
    pub fn save(&mut self) -> Result<()> {
        self.store.touch();
        let header_bytes = canonical_json(&self.header)?;
        let mut payload = canonical_json(&self.store)?;
        let nonce = random_array::<NONCE_LEN>()?;
        let ct = crypto::seal(
            self.dek.expose_secret(),
            &nonce,
            &aad_payload(&VAULT_MAGIC, FORMAT_VERSION, &header_bytes),
            &payload,
        )?;
        payload.zeroize();
        self.payload_nonce = nonce;
        self.payload_ct = ct;
        self.write_envelope()
    }

    fn write_envelope(&self) -> Result<()> {
        let envelope = Envelope {
            magic: VAULT_MAGIC,
            version: FORMAT_VERSION,
            header: canonical_json(&self.header)?,
            kdf: canonical_json(&self.kdf)?,
            dek_nonce: self.dek_nonce,
            wrapped_dek: self.wrapped_dek.clone(),
            payload_nonce: self.payload_nonce,
            payload_ct: self.payload_ct.clone(),
        };
        rotate_backups(&self.data_dir)?;
        let bytes = envelope.encode();
        let path = self.data_dir.vault_file();
        atomic_write(&path, &bytes)?;
        // Relecture : le fichier écrit doit être identique, sinon on le signale (sauvegarde intacte).
        if store::read_file(&path)? != bytes {
            return Err(Error::BadFormat("écriture du coffre non vérifiée"));
        }
        Ok(())
    }

    /// Déchiffre un secret à la demande (backend uniquement). Le résultat est
    /// un SecretString : Debug rédige, mémoire effacée au drop.
    pub fn secret(&self, entry_id: &str, field: SecretField) -> Result<SecretString> {
        let enc = enc_for(&self.store, entry_id, field)?;
        let nonce = b64_decode(&enc.nonce)?;
        let nonce: [u8; NONCE_LEN] = nonce
            .try_into()
            .map_err(|_| Error::BadFormat("nonce de taille invalide"))?;
        let ct = b64_decode(&enc.ct)?;
        let pt = crypto::open(
            self.dek.expose_secret(),
            &nonce,
            &aad_secret(&self.store.vault_id, entry_id, field.as_str()),
            &ct,
        )?;
        match String::from_utf8(pt) {
            Ok(s) => Ok(SecretString::from(s)),
            Err(err) => {
                let mut bytes = err.into_bytes();
                bytes.zeroize();
                Err(Error::Tampered)
            }
        }
    }

    /// (Re)chiffre un secret saisi par l'utilisateur. L'appelant fait `save()`.
    pub fn set_secret(
        &mut self,
        entry_id: &str,
        field: SecretField,
        value: &SecretString,
    ) -> Result<()> {
        let nonce = random_array::<NONCE_LEN>()?;
        let ct = crypto::seal(
            self.dek.expose_secret(),
            &nonce,
            &aad_secret(&self.store.vault_id, entry_id, field.as_str()),
            value.expose_secret().as_bytes(),
        )?;
        let enc = EncSecret::new(b64_encode(&nonce), b64_encode(&ct));
        match field {
            SecretField::MacroContent => {
                let m = self
                    .store
                    .macros
                    .iter_mut()
                    .find(|m| m.id == entry_id)
                    .ok_or(Error::NotFound("macro"))?;
                m.content = enc;
            }
            _ => {
                let c = self.store.credential_mut(entry_id)?;
                match field {
                    SecretField::Password => c.password = Some(enc),
                    SecretField::PrivateKey => c.private_key = Some(enc),
                    SecretField::Passphrase => c.passphrase = Some(enc),
                    SecretField::MacroContent => unreachable!(),
                }
                c.needs_reentry.retain(|f| f != field.as_str());
                c.updated_at = now_rfc3339();
            }
        }
        Ok(())
    }

    /// Active/désactive le cache keyring (opt-in, non bloquant).
    pub fn keyring_set_enabled(&mut self, enabled: bool) -> Result<()> {
        self.store.settings.keyring_cache = enabled;
        if enabled {
            keyring_cache::store(&self.store.vault_id, &self.dek);
        } else {
            keyring_cache::delete(&self.store.vault_id);
        }
        self.save()?;
        audit::append(
            &self.data_dir,
            if enabled {
                "keyring_cache_on"
            } else {
                "keyring_cache_off"
            },
            Some(&self.store.vault_id),
            None,
        )?;
        Ok(())
    }

    pub fn keyring_available(&self) -> bool {
        keyring_cache::available(&self.store.vault_id)
    }

    /// Efface un champ secret (l'utilisateur le retire explicitement) et le
    /// marque « à ressaisir ». L'appelant fait `save()`.
    pub fn clear_secret(&mut self, entry_id: &str, field: SecretField) -> Result<()> {
        if field == SecretField::MacroContent {
            return Err(Error::InvalidParams(
                "le contenu d'une macro ne peut pas être vide".into(),
            ));
        }
        let c = self.store.credential_mut(entry_id)?;
        match field {
            SecretField::Password => c.password = None,
            SecretField::PrivateKey => c.private_key = None,
            SecretField::Passphrase => c.passphrase = None,
            SecretField::MacroContent => unreachable!(),
        }
        let name = field.as_str().to_string();
        if !c.needs_reentry.contains(&name) {
            c.needs_reentry.push(name);
        }
        c.updated_at = now_rfc3339();
        Ok(())
    }

    /// Re-calibre les paramètres Argon2id sur la machine courante et re-wrap
    /// la DEK (payload inchangé, comme le changement de master).
    pub fn recalibrate_kdf(&mut self, master: &SecretString) -> Result<()> {
        let kdf_bytes = canonical_json(&self.kdf)?;
        let kek = derive_kek(master.expose_secret().as_bytes(), &self.kdf)?;
        let _ = unwrap_dek(
            &kek,
            &self.dek_nonce,
            &self.wrapped_dek,
            &self.header.vault_id,
            &kdf_bytes,
        )
        .map_err(|_| Error::WrongPassword)?;
        let kdf = KdfParams::new_calibrated()?;
        let new_bytes = canonical_json(&kdf)?;
        let kek_new = derive_kek(master.expose_secret().as_bytes(), &kdf)?;
        let (nonce, wrapped) = wrap_dek(&kek_new, &self.dek, &self.header.vault_id, &new_bytes)?;
        self.kdf = kdf;
        self.dek_nonce = nonce;
        self.wrapped_dek = wrapped;
        self.write_envelope()?;
        audit::append(
            &self.data_dir,
            "kdf_recalibrate",
            Some(&self.store.vault_id),
            None,
        )?;
        Ok(())
    }

    pub fn data_dir(&self) -> &DataDir {
        &self.data_dir
    }

    pub fn vault_id(&self) -> &str {
        &self.store.vault_id
    }

    pub fn dek(&self) -> &Key256 {
        &self.dek
    }
}

fn read_envelope(dd: &DataDir) -> Result<Envelope> {
    let path = dd.vault_file();
    if !path.exists() {
        return Err(Error::NotFound("coffre"));
    }
    Envelope::decode(&store::read_file(&path)?)
}

fn enc_for<'a>(store: &'a Store, entry_id: &str, field: SecretField) -> Result<&'a EncSecret> {
    match field {
        SecretField::MacroContent => Ok(&store.macro_by_id(entry_id)?.content),
        _ => {
            let c = store.credential(entry_id)?;
            match field {
                SecretField::Password => c.password.as_ref(),
                SecretField::PrivateKey => c.private_key.as_ref(),
                SecretField::Passphrase => c.passphrase.as_ref(),
                SecretField::MacroContent => unreachable!(),
            }
            .ok_or(Error::NotFound("champ secret"))
        }
    }
}

/// Vérification eager à l'ouverture : chaque secret doit se déchiffrer avec
/// son AAD (détecte permutation/corruption avant tout usage).
fn verify_secrets(store: &Store, dek: &Key256, container_id: &str) -> Result<()> {
    let check = |entry_id: &str, enc: &EncSecret, field: SecretField| -> Result<()> {
        let nonce = b64_decode(&enc.nonce)?;
        let nonce: [u8; NONCE_LEN] = nonce
            .try_into()
            .map_err(|_| Error::BadFormat("nonce de taille invalide"))?;
        let ct = b64_decode(&enc.ct)?;
        let mut pt = crypto::open(
            dek.expose_secret(),
            &nonce,
            &aad_secret(container_id, entry_id, field.as_str()),
            &ct,
        )?;
        pt.zeroize();
        Ok(())
    };
    for c in &store.credentials {
        if let Some(enc) = &c.password {
            check(&c.id, enc, SecretField::Password)?;
        }
        if let Some(enc) = &c.private_key {
            check(&c.id, enc, SecretField::PrivateKey)?;
        }
        if let Some(enc) = &c.passphrase {
            check(&c.id, enc, SecretField::Passphrase)?;
        }
    }
    for m in &store.macros {
        check(&m.id, &m.content, SecretField::MacroContent)?;
    }
    Ok(())
}
