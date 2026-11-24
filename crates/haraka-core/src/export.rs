//! Export portable et import fusionnel (EXPORT_FORMAT.md §6–7, critère n°1).
//!
//! Export : re-chiffrement de chaque secret avec une DEK d'export dérivée de la
//! passphrase d'export (distincte du mot de passe maître). Import : vérification
//! d'intégrité, aperçu des conflits UUID, stratégies garder/remplacer/dupliquer,
//! re-chiffrement avec la DEK du coffre local, fusion transactionnelle.

use std::collections::HashMap;
use std::path::Path;

use secrecy::{ExposeSecret, SecretBox, SecretString};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::crypto::{
    self, DEK_LEN, KdfParams, Key256, NONCE_LEN, aad_payload, aad_secret, b64_decode, b64_encode,
    canonical_json, derive_kek, now_rfc3339, random_array, unwrap_dek, wrap_dek,
};
use crate::error::{Error, Result};
use crate::format::{
    APP_NAME, ContentManifest, Counts, EXPORT_MAGIC, Envelope, ExportHeader, FORMAT_VERSION,
    PAYLOAD_ALG,
};
use crate::model::{Connection, Credential, EncSecret, Folder, Macro, SCHEMA_VERSION, Store};
use crate::store::atomic_write;
use crate::vault::{SecretField, Vault};
use crate::{audit, effective};

#[derive(Clone, Debug)]
pub enum ExportScope {
    All,
    Folder(String),
    NoSecrets,
}

impl ExportScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Folder(_) => "folder",
            Self::NoSecrets => "no-secrets",
        }
    }

    pub fn includes_secrets(&self) -> bool {
        !matches!(self, Self::NoSecrets)
    }

    pub fn parse(scope: &str, folder_id: Option<String>) -> Result<Self> {
        match scope {
            "all" => Ok(Self::All),
            "no-secrets" => Ok(Self::NoSecrets),
            "folder" => {
                Ok(Self::Folder(folder_id.ok_or_else(|| {
                    Error::InvalidParams("folder_id requis".into())
                })?))
            }
            _ => Err(Error::InvalidParams(format!("portée inconnue : {scope}"))),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportPayload {
    pub schema_version: u32,
    pub export_id: String,
    pub source_vault_id: String,
    pub folders: Vec<Folder>,
    pub connections: Vec<Connection>,
    pub credentials: Vec<Credential>,
    pub macros: Vec<Macro>,
}

/// Exporte un bundle `.hrkx`. `kdf` : None → calibration machine (0,5–1 s).
pub fn export_bundle(
    vault: &Vault,
    scope: &ExportScope,
    passphrase: &SecretString,
    kdf: Option<KdfParams>,
    out: &Path,
) -> Result<Counts> {
    let kdf = match kdf {
        Some(k) => k,
        None => KdfParams::new_calibrated()?,
    };
    let export_id = Uuid::new_v4().to_string();
    let export_dek: Key256 = SecretBox::new(Box::new(random_array::<DEK_LEN>()?));
    let includes = scope.includes_secrets();

    let (folders, connections, mut credentials, mut macros) = select(vault, scope)?;

    for c in &mut credentials {
        for field in [
            SecretField::Password,
            SecretField::PrivateKey,
            SecretField::Passphrase,
        ] {
            let present = match field {
                SecretField::Password => c.password.is_some(),
                SecretField::PrivateKey => c.private_key.is_some(),
                SecretField::Passphrase => c.passphrase.is_some(),
                SecretField::MacroContent => false,
            };
            if !present {
                continue;
            }
            if includes {
                let pt = vault.secret(&c.id, field)?;
                let nonce = random_array::<NONCE_LEN>()?;
                let ct = crypto::seal(
                    export_dek.expose_secret(),
                    &nonce,
                    &aad_secret(&export_id, &c.id, field.as_str()),
                    pt.expose_secret().as_bytes(),
                )?;
                let enc = EncSecret::new(b64_encode(&nonce), b64_encode(&ct));
                match field {
                    SecretField::Password => c.password = Some(enc),
                    SecretField::PrivateKey => c.private_key = Some(enc),
                    SecretField::Passphrase => c.passphrase = Some(enc),
                    SecretField::MacroContent => unreachable!(),
                }
            } else {
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
            }
        }
    }

    if includes {
        for m in &mut macros {
            let pt = vault.secret(&m.id, SecretField::MacroContent)?;
            let nonce = random_array::<NONCE_LEN>()?;
            let ct = crypto::seal(
                export_dek.expose_secret(),
                &nonce,
                &aad_secret(&export_id, &m.id, SecretField::MacroContent.as_str()),
                pt.expose_secret().as_bytes(),
            )?;
            m.content = EncSecret::new(b64_encode(&nonce), b64_encode(&ct));
        }
    } else {
        macros.clear();
    }

    let counts = Counts {
        connections: connections.len(),
        credentials: credentials.len(),
        folders: folders.len(),
        macros: macros.len(),
    };
    let header = ExportHeader {
        app: APP_NAME.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        content: ContentManifest {
            counts: counts.clone(),
            folder_id: match scope {
                ExportScope::Folder(id) => Some(id.clone()),
                _ => None,
            },
            includes_secrets: includes,
            scope: scope.as_str().to_string(),
        },
        export_id: export_id.clone(),
        exported_at: now_rfc3339(),
        payload_alg: PAYLOAD_ALG.to_string(),
        source_vault_id: vault.vault_id().to_string(),
    };
    let payload_obj = ExportPayload {
        schema_version: SCHEMA_VERSION,
        export_id: export_id.clone(),
        source_vault_id: vault.vault_id().to_string(),
        folders,
        connections,
        credentials,
        macros,
    };

    let header_bytes = canonical_json(&header)?;
    let kdf_bytes = canonical_json(&kdf)?;
    let mut payload = canonical_json(&payload_obj)?;
    let payload_nonce = random_array::<NONCE_LEN>()?;
    let payload_ct = crypto::seal(
        export_dek.expose_secret(),
        &payload_nonce,
        &aad_payload(&EXPORT_MAGIC, FORMAT_VERSION, &header_bytes),
        &payload,
    )?;
    payload.zeroize();

    let kek = derive_kek(passphrase.expose_secret().as_bytes(), &kdf)?;
    let (dek_nonce, wrapped_dek) = wrap_dek(&kek, &export_dek, &export_id, &kdf_bytes)?;
    let envelope = Envelope {
        magic: EXPORT_MAGIC,
        version: FORMAT_VERSION,
        header: header_bytes,
        kdf: kdf_bytes,
        dek_nonce,
        wrapped_dek,
        payload_nonce,
        payload_ct,
    };
    atomic_write(out, &envelope.encode())?;
    audit::append(
        vault.data_dir(),
        "export",
        Some(&export_id),
        Some(&format!("scope={}, secrets={}", scope.as_str(), includes)),
    )?;
    Ok(counts)
}

/// Sélection du périmètre exporté (clones ; le store local n'est pas modifié).
type Selection = (Vec<Folder>, Vec<Connection>, Vec<Credential>, Vec<Macro>);

fn select(vault: &Vault, scope: &ExportScope) -> Result<Selection> {
    let store = &vault.store;
    match scope {
        ExportScope::All | ExportScope::NoSecrets => Ok((
            store.folders.clone(),
            store.connections.clone(),
            store.credentials.clone(),
            store.macros.clone(),
        )),
        ExportScope::Folder(root) => {
            store.folder(root)?;
            let folder_ids = effective::subtree_folders(store, root);
            let mut folders: Vec<Folder> = store
                .folders
                .iter()
                .filter(|f| folder_ids.contains(&f.id))
                .cloned()
                .collect();
            // La racine exportée n'a pas de parent dans le bundle (§5.2).
            for f in &mut folders {
                if f.parent_id
                    .as_deref()
                    .is_some_and(|p| !folder_ids.contains(&p.to_string()))
                {
                    f.parent_id = None;
                }
            }
            let connections: Vec<Connection> = store
                .connections
                .iter()
                .filter(|c| folder_ids.contains(&c.folder_id))
                .cloned()
                .collect();
            let mut cred_ids: Vec<String> = Vec::new();
            let mut macro_ids: Vec<String> = Vec::new();
            for conn in &connections {
                if let Some(id) = effective::credential_for(store, conn)
                    && !cred_ids.contains(&id)
                {
                    cred_ids.push(id);
                }
                let chain = conn.jump_chain.clone().unwrap_or_else(|| {
                    effective::folder_chain(store, &conn.folder_id)
                        .iter()
                        .rev()
                        .find_map(|f| {
                            if f.defaults.jump_chain.is_empty() {
                                None
                            } else {
                                Some(f.defaults.jump_chain.clone())
                            }
                        })
                        .unwrap_or_default()
                });
                for hop in chain {
                    if let Some(id) = hop.credential_id
                        && !cred_ids.contains(&id)
                    {
                        cred_ids.push(id);
                    }
                }
                if let Some(id) = &conn.startup_snippet_id
                    && !macro_ids.contains(id)
                {
                    macro_ids.push(id.clone());
                }
            }
            let credentials = store
                .credentials
                .iter()
                .filter(|c| cred_ids.contains(&c.id))
                .cloned()
                .collect();
            let macros = store
                .macros
                .iter()
                .filter(|m| macro_ids.contains(&m.id))
                .cloned()
                .collect();
            Ok((folders, connections, credentials, macros))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    Keep,
    Replace,
    Duplicate,
}

impl Strategy {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "keep" => Ok(Self::Keep),
            "replace" => Ok(Self::Replace),
            "duplicate" => Ok(Self::Duplicate),
            _ => Err(Error::InvalidParams(format!("stratégie inconnue : {s}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictKind {
    Folder,
    Connection,
    Credential,
    Macro,
}

impl ConflictKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Connection => "connection",
            Self::Credential => "credential",
            Self::Macro => "macro",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conflict {
    pub kind: ConflictKind,
    pub id: String,
    pub name: String,
}

/// Aperçu d'import : bundle déchiffré + vérifié, conflits UUID avec le coffre
/// local. La DEK d'export reste en mémoire backend le temps de l'application.
pub struct ImportPreview {
    pub id: String,
    pub header: ExportHeader,
    pub payload: ExportPayload,
    pub conflicts: Vec<Conflict>,
    export_dek: Key256,
}

impl ImportPreview {
    pub fn counts(&self) -> Counts {
        Counts {
            connections: self.payload.connections.len(),
            credentials: self.payload.credentials.len(),
            folders: self.payload.folders.len(),
            macros: self.payload.macros.len(),
        }
    }
}

pub fn preview(path: &Path, passphrase: &SecretString, local: &Store) -> Result<ImportPreview> {
    let envelope = Envelope::decode(&std::fs::read(path)?)?;
    if envelope.magic != EXPORT_MAGIC {
        return Err(Error::BadFormat("magic d'export attendu"));
    }
    let header: ExportHeader = envelope.header_json()?;
    let kdf = envelope.kdf_params()?;
    let kek = derive_kek(passphrase.expose_secret().as_bytes(), &kdf)?;
    let export_dek = unwrap_dek(
        &kek,
        &envelope.dek_nonce,
        &envelope.wrapped_dek,
        &header.export_id,
        &envelope.kdf,
    )
    .map_err(|_| Error::WrongPassword)?;
    let mut pt = crypto::open(
        export_dek.expose_secret(),
        &envelope.payload_nonce,
        &aad_payload(&envelope.magic, envelope.version, &envelope.header),
        &envelope.payload_ct,
    )?;
    let payload: ExportPayload = serde_json::from_slice(&pt)?;
    pt.zeroize();
    if payload.schema_version != SCHEMA_VERSION {
        return Err(Error::UnsupportedSchema(payload.schema_version));
    }
    if payload.export_id != header.export_id || payload.source_vault_id != header.source_vault_id {
        return Err(Error::Tampered);
    }
    verify_export_secrets(&payload, &export_dek, &header.export_id)?;
    let conflicts = compute_conflicts(&payload, local);
    Ok(ImportPreview {
        id: Uuid::new_v4().to_string(),
        header,
        payload,
        conflicts,
        export_dek,
    })
}

fn verify_export_secrets(payload: &ExportPayload, dek: &Key256, export_id: &str) -> Result<()> {
    let check = |entry_id: &str, enc: &EncSecret, field: SecretField| -> Result<()> {
        let nonce: [u8; NONCE_LEN] = b64_decode(&enc.nonce)?
            .try_into()
            .map_err(|_| Error::BadFormat("nonce de taille invalide"))?;
        let ct = b64_decode(&enc.ct)?;
        let mut pt = crypto::open(
            dek.expose_secret(),
            &nonce,
            &aad_secret(export_id, entry_id, field.as_str()),
            &ct,
        )?;
        pt.zeroize();
        Ok(())
    };
    for c in &payload.credentials {
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
    for m in &payload.macros {
        check(&m.id, &m.content, SecretField::MacroContent)?;
    }
    Ok(())
}

fn compute_conflicts(payload: &ExportPayload, local: &Store) -> Vec<Conflict> {
    let mut out = Vec::new();
    for f in &payload.folders {
        if local.folders.iter().any(|x| x.id == f.id) {
            out.push(Conflict {
                kind: ConflictKind::Folder,
                id: f.id.clone(),
                name: f.name.clone(),
            });
        }
    }
    for c in &payload.connections {
        if local.connections.iter().any(|x| x.id == c.id) {
            out.push(Conflict {
                kind: ConflictKind::Connection,
                id: c.id.clone(),
                name: c.name.clone(),
            });
        }
    }
    for c in &payload.credentials {
        if local.credentials.iter().any(|x| x.id == c.id) {
            out.push(Conflict {
                kind: ConflictKind::Credential,
                id: c.id.clone(),
                name: c.name.clone(),
            });
        }
    }
    for m in &payload.macros {
        if local.macros.iter().any(|x| x.id == m.id) {
            out.push(Conflict {
                kind: ConflictKind::Macro,
                id: m.id.clone(),
                name: m.name.clone(),
            });
        }
    }
    out
}

enum Decision {
    Skip,
    Replace,
    Insert,
}

fn decide(
    kind: ConflictKind,
    id: &str,
    exists: bool,
    default: Strategy,
    overrides: &HashMap<String, Strategy>,
) -> Decision {
    if !exists {
        return Decision::Insert;
    }
    let strategy = overrides
        .get(&format!("{}:{id}", kind.as_str()))
        .copied()
        .unwrap_or(default);
    match strategy {
        Strategy::Keep => Decision::Skip,
        Strategy::Replace => Decision::Replace,
        Strategy::Duplicate => Decision::Insert,
    }
}

/// Applique l'import : fusion par UUID, re-chiffrement des secrets avec la DEK
/// locale, remappage des références pour les entrées dupliquées.
/// Transactionnel : le store local n'est remplacé qu'après succès complet.
pub fn apply(
    vault: &mut Vault,
    preview: ImportPreview,
    default_strategy: Strategy,
    overrides: &HashMap<String, Strategy>,
) -> Result<Counts> {
    let export_id = preview.header.export_id.clone();
    let mut merged = vault.store.clone();
    let mut counts = Counts::default();

    // Passe 1 : nouveaux UUID pour les entrées dupliquées.
    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut plan_duplicate = |kind: ConflictKind, id: &str, exists: bool| {
        if exists
            && overrides
                .get(&format!("{}:{id}", kind.as_str()))
                .copied()
                .unwrap_or(default_strategy)
                == Strategy::Duplicate
        {
            id_map.insert(id.to_string(), Uuid::new_v4().to_string());
        }
    };
    for f in &preview.payload.folders {
        plan_duplicate(
            ConflictKind::Folder,
            &f.id,
            merged.folders.iter().any(|x| x.id == f.id),
        );
    }
    for c in &preview.payload.connections {
        plan_duplicate(
            ConflictKind::Connection,
            &c.id,
            merged.connections.iter().any(|x| x.id == c.id),
        );
    }
    for c in &preview.payload.credentials {
        plan_duplicate(
            ConflictKind::Credential,
            &c.id,
            merged.credentials.iter().any(|x| x.id == c.id),
        );
    }
    for m in &preview.payload.macros {
        plan_duplicate(
            ConflictKind::Macro,
            &m.id,
            merged.macros.iter().any(|x| x.id == m.id),
        );
    }
    let remap = |id: &str| -> String { id_map.get(id).cloned().unwrap_or_else(|| id.to_string()) };
    let remap_opt = |id: &Option<String>| -> Option<String> { id.as_deref().map(remap) };

    // Passe 2 : dossiers.
    for mut f in preview.payload.folders {
        let original = f.id.clone();
        let exists = merged.folders.iter().any(|x| x.id == original);
        match decide(
            ConflictKind::Folder,
            &original,
            exists,
            default_strategy,
            overrides,
        ) {
            Decision::Skip => continue,
            Decision::Replace => {
                f.id = original.clone();
                f.parent_id = remap_opt(&f.parent_id);
                f.defaults.credential_id = remap_opt(&f.defaults.credential_id);
                for hop in &mut f.defaults.jump_chain {
                    hop.credential_id = remap_opt(&hop.credential_id);
                }
                let pos = merged
                    .folders
                    .iter()
                    .position(|x| x.id == original)
                    .unwrap();
                merged.folders[pos] = f;
                counts.folders += 1;
            }
            Decision::Insert => {
                f.id = remap(&original);
                f.parent_id = remap_opt(&f.parent_id);
                f.defaults.credential_id = remap_opt(&f.defaults.credential_id);
                for hop in &mut f.defaults.jump_chain {
                    hop.credential_id = remap_opt(&hop.credential_id);
                }
                merged.folders.push(f);
                counts.folders += 1;
            }
        }
    }

    // Passe 2 : credentials (re-chiffrement export → local).
    for mut c in preview.payload.credentials {
        let original = c.id.clone();
        let exists = merged.credentials.iter().any(|x| x.id == original);
        let decision = decide(
            ConflictKind::Credential,
            &original,
            exists,
            default_strategy,
            overrides,
        );
        if matches!(decision, Decision::Skip) {
            continue;
        }
        let new_id = if matches!(decision, Decision::Replace) {
            original.clone()
        } else {
            remap(&original)
        };
        for field in [
            SecretField::Password,
            SecretField::PrivateKey,
            SecretField::Passphrase,
        ] {
            let enc = match field {
                SecretField::Password => c.password.take(),
                SecretField::PrivateKey => c.private_key.take(),
                SecretField::Passphrase => c.passphrase.take(),
                SecretField::MacroContent => None,
            };
            let Some(enc) = enc else { continue };
            let reencrypted = reencrypt_secret(
                &preview.export_dek,
                &export_id,
                &original,
                vault.dek(),
                vault.vault_id(),
                &new_id,
                field,
                &enc,
            )?;
            match field {
                SecretField::Password => c.password = Some(reencrypted),
                SecretField::PrivateKey => c.private_key = Some(reencrypted),
                SecretField::Passphrase => c.passphrase = Some(reencrypted),
                SecretField::MacroContent => unreachable!(),
            }
        }
        c.id = new_id;
        c.updated_at = now_rfc3339();
        match decision {
            Decision::Replace => {
                let pos = merged
                    .credentials
                    .iter()
                    .position(|x| x.id == original)
                    .unwrap();
                merged.credentials[pos] = c;
            }
            _ => merged.credentials.push(c),
        }
        counts.credentials += 1;
    }

    // Passe 2 : connexions.
    for mut c in preview.payload.connections {
        let original = c.id.clone();
        let exists = merged.connections.iter().any(|x| x.id == original);
        match decide(
            ConflictKind::Connection,
            &original,
            exists,
            default_strategy,
            overrides,
        ) {
            Decision::Skip => continue,
            Decision::Replace => {
                c.id = original.clone();
                remap_connection(&mut c, &remap_opt);
                let pos = merged
                    .connections
                    .iter()
                    .position(|x| x.id == original)
                    .unwrap();
                merged.connections[pos] = c;
                counts.connections += 1;
            }
            Decision::Insert => {
                c.id = remap(&original);
                remap_connection(&mut c, &remap_opt);
                merged.connections.push(c);
                counts.connections += 1;
            }
        }
    }

    // Passe 2 : macros.
    for mut m in preview.payload.macros {
        let original = m.id.clone();
        let exists = merged.macros.iter().any(|x| x.id == original);
        let decision = decide(
            ConflictKind::Macro,
            &original,
            exists,
            default_strategy,
            overrides,
        );
        if matches!(decision, Decision::Skip) {
            continue;
        }
        let new_id = if matches!(decision, Decision::Replace) {
            original.clone()
        } else {
            remap(&original)
        };
        m.content = reencrypt_secret(
            &preview.export_dek,
            &export_id,
            &original,
            vault.dek(),
            vault.vault_id(),
            &new_id,
            SecretField::MacroContent,
            &m.content,
        )?;
        m.id = new_id;
        match decision {
            Decision::Replace => {
                let pos = merged.macros.iter().position(|x| x.id == original).unwrap();
                merged.macros[pos] = m;
            }
            _ => merged.macros.push(m),
        }
        counts.macros += 1;
    }

    // Aucune erreur jusqu'ici → on remplace le store et on écrit atomiquement.
    vault.store = merged;
    vault.save()?;
    audit::append(
        vault.data_dir(),
        "import",
        Some(&preview.header.source_vault_id),
        Some(&format!(
            "strategy={:?}, folders={}, connections={}, credentials={}, macros={}",
            default_strategy, counts.folders, counts.connections, counts.credentials, counts.macros
        )),
    )?;
    Ok(counts)
}

fn remap_connection(c: &mut Connection, remap_opt: &impl Fn(&Option<String>) -> Option<String>) {
    c.folder_id = remap_opt(&Some(c.folder_id.clone())).unwrap_or_default();
    c.credential_id = remap_opt(&c.credential_id);
    c.startup_snippet_id = remap_opt(&c.startup_snippet_id);
    if let Some(chain) = &mut c.jump_chain {
        for hop in chain {
            hop.credential_id = remap_opt(&hop.credential_id);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn reencrypt_secret(
    export_dek: &Key256,
    export_id: &str,
    original_entry_id: &str,
    local_dek: &Key256,
    local_vault_id: &str,
    new_entry_id: &str,
    field: SecretField,
    enc: &EncSecret,
) -> Result<EncSecret> {
    let nonce: [u8; NONCE_LEN] = b64_decode(&enc.nonce)?
        .try_into()
        .map_err(|_| Error::BadFormat("nonce de taille invalide"))?;
    let ct = b64_decode(&enc.ct)?;
    let mut pt = crypto::open(
        export_dek.expose_secret(),
        &nonce,
        &aad_secret(export_id, original_entry_id, field.as_str()),
        &ct,
    )?;
    let new_nonce = random_array::<NONCE_LEN>()?;
    let new_ct = crypto::seal(
        local_dek.expose_secret(),
        &new_nonce,
        &aad_secret(local_vault_id, new_entry_id, field.as_str()),
        &pt,
    )?;
    pt.zeroize();
    Ok(EncSecret::new(b64_encode(&new_nonce), b64_encode(&new_ct)))
}
