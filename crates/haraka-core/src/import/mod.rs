//! Imports depuis d'autres outils (M7) : `~/.ssh/config`, Remmina, MobaXterm.
//!
//! Règle : **un import ne plante jamais**. Les parseurs sont purs (texte → plan)
//! et tolérants ; l'applicateur crée dossier « Import <source> », credentials,
//! connexions, en respectant la validation du modèle, sans rien modifier ni
//! supprimer d'existant. Les avertissements sont structurés `{code, params}` :
//! l'UI les traduit (`import.warn.<code>`).

use std::collections::{BTreeMap, HashMap, HashSet};

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::manage;
use crate::model::{
    Connection, ConnectionDefaults, Folder, JumpHop, SftpOptions, Store, TunnelSpec,
};
use crate::vault::{SecretField, Vault};

pub mod mobaxterm;
pub mod remmina;
pub mod ssh_config;

/// Taille maximale d'entrée traitée (au-delà : tronquée, avertissement).
pub const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
/// Ligne plus longue : ignorée (avertissement).
pub const MAX_LINE_CHARS: usize = 16 * 1024;
/// Plafond d'avertissements conservés dans un plan.
pub const MAX_WARNINGS: usize = 200;
/// Plafond de connexions créées par un import.
pub const MAX_CONNECTIONS: usize = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportSource {
    SshConfig,
    Remmina,
    Mobaxterm,
}

impl ImportSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SshConfig => "ssh_config",
            Self::Remmina => "remmina",
            Self::Mobaxterm => "mobaxterm",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "ssh_config" => Ok(Self::SshConfig),
            "remmina" => Ok(Self::Remmina),
            "mobaxterm" => Ok(Self::Mobaxterm),
            other => Err(Error::InvalidParams(format!(
                "source d'import inconnue : {other}"
            ))),
        }
    }

    /// Nom du dossier racine créé pour la revue.
    pub fn root_folder_name(self) -> &'static str {
        match self {
            Self::SshConfig => "Import ssh_config",
            Self::Remmina => "Import Remmina",
            Self::Mobaxterm => "Import MobaXterm",
        }
    }
}

/// Avertissement structuré, traduit côté UI (`import.warn.<code>`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportWarning {
    pub code: String,
    pub params: BTreeMap<String, String>,
}

impl ImportWarning {
    pub fn new(code: &str, params: &[(&str, &str)]) -> Self {
        Self {
            code: code.to_string(),
            params: params
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.to_string()))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanHop {
    pub host: String,
    pub port: Option<u16>,
    pub user: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanTunnel {
    /// `local` | `remote` | `dynamic`
    pub kind: String,
    pub bind_addr: String,
    pub bind_port: u16,
    pub dest_host: Option<String>,
    pub dest_port: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDefaults {
    pub user: Option<String>,
    pub port: Option<u16>,
    pub keepalive_s: Option<u32>,
    pub jump_chain: Vec<PlanHop>,
    /// Clé d'un `PlanCredential`.
    pub credential: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanConnection {
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    /// Chemin de sous-dossiers sous la racine d'import (vide = racine).
    pub folder_path: Vec<String>,
    pub credential: Option<String>,
    /// `None` = hérite ; `Some(vec![])` = aucun saut explicite.
    pub jump_chain: Option<Vec<PlanHop>>,
    pub keepalive_s: Option<u32>,
    pub tunnels: Vec<PlanTunnel>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanCredential {
    pub key: String,
    pub name: String,
    pub username: String,
    /// Fichier de clé privée à lire côté appelant (chemin déjà développé).
    pub key_file: Option<String>,
    /// Un mot de passe existait dans la source mais n'est pas importable.
    pub password_needs_reentry: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPlan {
    pub source: ImportSource,
    pub root_defaults: PlanDefaults,
    pub folders: Vec<Vec<String>>,
    pub connections: Vec<PlanConnection>,
    pub credentials: Vec<PlanCredential>,
    pub warnings: Vec<ImportWarning>,
    pub skipped: usize,
}

impl ImportPlan {
    pub fn new(source: ImportSource) -> Self {
        Self {
            source,
            root_defaults: PlanDefaults::default(),
            folders: Vec::new(),
            connections: Vec::new(),
            credentials: Vec::new(),
            warnings: Vec::new(),
            skipped: 0,
        }
    }

    pub fn warn(&mut self, code: &str, params: &[(&str, &str)]) {
        self.push_warning(ImportWarning::new(code, params));
    }

    pub fn push_warning(&mut self, w: ImportWarning) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(w);
        } else if self.warnings.len() == MAX_WARNINGS {
            self.warnings
                .push(ImportWarning::new("warnings_truncated", &[]));
        }
    }

    /// Élément ignoré (compté dans `skipped`) + avertissement.
    pub fn skip(&mut self, code: &str, params: &[(&str, &str)]) {
        self.skipped += 1;
        self.warn(code, params);
    }

    pub fn add_credential(&mut self, cred: PlanCredential) {
        if !self.credentials.iter().any(|c| c.key == cred.key) {
            self.credentials.push(cred);
        }
    }

    pub fn add_folder_path(&mut self, path: &[String]) {
        for n in 1..=path.len() {
            let p = path[..n].to_vec();
            if !self.folders.contains(&p) {
                self.folders.push(p);
            }
        }
    }
}

// ------------------------------------------------------------ texte & INI

/// Décode des octets en texte : BOM UTF-8/UTF-16 gérés, UTF-8 invalide → lossy
/// (avertissement), données binaires (NUL) → texte vide (avertissement).
pub fn decode_text(bytes: &[u8]) -> (String, Vec<ImportWarning>) {
    let mut warnings = Vec::new();
    let mut bytes = bytes;
    if bytes.len() > MAX_INPUT_BYTES {
        bytes = &bytes[..MAX_INPUT_BYTES];
        warnings.push(ImportWarning::new("input_truncated", &[]));
    }
    let text = if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let be = bytes[0] == 0xFE;
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if be {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        match std::str::from_utf8(b) {
            Ok(s) => s.to_string(),
            Err(_) => {
                warnings.push(ImportWarning::new("encoding_lossy", &[]));
                String::from_utf8_lossy(b).into_owned()
            }
        }
    };
    if text.contains('\0') {
        warnings.push(ImportWarning::new("binary_input", &[]));
        return (String::new(), warnings);
    }
    (text, warnings)
}

/// Lignes (numéro 1-based, contenu) en écartant celles trop longues.
pub(crate) fn numbered_lines<'a>(text: &'a str, plan: &mut ImportPlan) -> Vec<(usize, &'a str)> {
    let mut out = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let line = raw.trim_end_matches('\r');
        let line = if i == 0 {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        if line.chars().take(MAX_LINE_CHARS + 1).count() > MAX_LINE_CHARS {
            plan.warn("line_too_long", &[("line", &(i + 1).to_string())]);
            continue;
        }
        out.push((i + 1, line));
    }
    out
}

pub(crate) struct IniSection {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

/// INI tolérant : `[section]`, `clé=valeur` (premier `=`), commentaires `;`/`#`.
pub(crate) fn parse_ini(text: &str, plan: &mut ImportPlan) -> Vec<IniSection> {
    let mut sections: Vec<IniSection> = Vec::new();
    for (_, line) in numbered_lines(text, plan) {
        let l = line.trim();
        if l.is_empty() || l.starts_with(';') || l.starts_with('#') {
            continue;
        }
        if let Some(rest) = l.strip_prefix('[') {
            let name = rest.split(']').next().unwrap_or("").trim().to_string();
            sections.push(IniSection {
                name,
                entries: Vec::new(),
            });
        } else if let Some((k, v)) = l.split_once('=') {
            if sections.is_empty() {
                sections.push(IniSection {
                    name: String::new(),
                    entries: Vec::new(),
                });
            }
            if let Some(s) = sections.last_mut() {
                s.entries.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    sections
}

/// `host[:port]`, `[v6]:port`, `host` (IPv6 nu sans crochets = pas de port).
pub(crate) fn split_host_port(s: &str) -> (String, Option<u16>) {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('[')
        && let Some((h, tail)) = rest.split_once(']')
    {
        let port = tail.strip_prefix(':').and_then(|p| p.parse().ok());
        return (h.to_string(), port);
    }
    if s.matches(':').count() == 1
        && let Some((h, p)) = s.split_once(':')
    {
        return match p.parse::<u16>() {
            Ok(p) => (h.to_string(), Some(p)),
            Err(_) => (s.to_string(), None),
        };
    }
    (s.to_string(), None)
}

/// Supprime les caractères de contrôle, borne la longueur.
pub(crate) fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect::<String>()
        .trim()
        .to_string()
}

pub(crate) fn file_stem(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.')
        .map(|(a, _)| a)
        .filter(|a| !a.is_empty())
        .unwrap_or(name)
        .to_string()
}

// ------------------------------------------------------------ applicateur

/// Matériel de clé lu par l'appelant (le noyau ne touche pas au système de fichiers).
pub struct KeyMaterial {
    pub pem: SecretString,
    /// Clé chiffrée dont la passphrase est inconnue → `needs_reentry`.
    pub encrypted: bool,
}

pub struct PendingSecret {
    pub credential_id: String,
    pub field: SecretField,
    pub value: SecretString,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub source: ImportSource,
    pub root_folder: String,
    pub imported: usize,
    pub folders: usize,
    pub credentials: usize,
    pub skipped: usize,
    pub secrets_to_reenter: usize,
    pub warnings: Vec<ImportWarning>,
}

fn reentry_fields(pc: &PlanCredential, keys: &HashMap<String, KeyMaterial>) -> Vec<&'static str> {
    let mut v = Vec::new();
    if pc.password_needs_reentry {
        v.push("password");
    }
    if let Some(kf) = &pc.key_file {
        match keys.get(kf) {
            Some(k) if k.encrypted => v.push("passphrase"),
            Some(_) => {}
            None => v.push("private_key"),
        }
    }
    v
}

fn unique_name(base: &str, used: &mut HashSet<String>) -> String {
    let mut name = base.to_string();
    let mut n = 2;
    while used.contains(&name.to_lowercase()) {
        name = format!("{base} ({n})");
        n += 1;
    }
    used.insert(name.to_lowercase());
    name
}

fn hops(plan_hops: &[PlanHop]) -> Vec<JumpHop> {
    plan_hops
        .iter()
        .filter(|h| !clean(&h.host).is_empty())
        .map(|h| JumpHop {
            host: clean(&h.host),
            port: h.port,
            user: h.user.as_deref().map(clean).filter(|u| !u.is_empty()),
            credential_id: None,
        })
        .collect()
}

/// Applique le plan sur le store (sans toucher aux secrets : ils sont renvoyés
/// pour que l'appelant les chiffre via le coffre). N'altère aucun élément existant.
pub fn apply_to_store(
    store: &mut Store,
    plan: &ImportPlan,
    keys: &HashMap<String, KeyMaterial>,
) -> Result<(ImportReport, Vec<PendingSecret>)> {
    let mut warnings = plan.warnings.clone();
    let mut skipped = plan.skipped;
    let mut secrets = Vec::new();

    let mut top: HashSet<String> = store
        .folders
        .iter()
        .filter(|f| f.parent_id.is_none())
        .map(|f| f.name.to_lowercase())
        .collect();
    let root_name = unique_name(plan.source.root_folder_name(), &mut top);
    let root_id = Uuid::new_v4().to_string();

    // Credentials.
    let mut cred_names: HashSet<String> = store
        .credentials
        .iter()
        .map(|c| c.name.to_lowercase())
        .collect();
    let mut cred_ids: HashMap<String, String> = HashMap::new();
    let mut reenter = 0usize;
    for pc in &plan.credentials {
        let name = unique_name(
            &if clean(&pc.name).is_empty() {
                "import".to_string()
            } else {
                clean(&pc.name)
            },
            &mut cred_names,
        );
        let id = manage::upsert_credential_meta(store, None, &name, &clean(&pc.username))?;
        let fields = reentry_fields(pc, keys);
        let cred = store.credential_mut(&id)?;
        cred.key_origin = pc.key_file.as_ref().map(|_| "imported".to_string());
        cred.needs_reentry = fields.iter().map(|f| (*f).to_string()).collect();
        if !fields.is_empty() {
            reenter += 1;
        }
        if let Some(kf) = &pc.key_file
            && let Some(k) = keys.get(kf)
        {
            secrets.push(PendingSecret {
                credential_id: id.clone(),
                field: SecretField::PrivateKey,
                value: SecretString::from(k.pem.expose_secret().to_string()),
            });
        }
        cred_ids.insert(pc.key.clone(), id);
    }

    // Dossier racine.
    let d = &plan.root_defaults;
    let sort_order = store
        .folders
        .iter()
        .filter(|f| f.parent_id.is_none())
        .map(|f| f.sort_order)
        .max()
        .map_or(0, |m| m.saturating_add(1));
    manage::upsert_folder(
        store,
        Folder {
            id: root_id.clone(),
            parent_id: None,
            name: root_name.clone(),
            color: None,
            icon: None,
            sort_order,
            tags: Vec::new(),
            defaults: ConnectionDefaults {
                user: d.user.as_deref().map(clean).filter(|u| !u.is_empty()),
                port: d.port,
                credential_id: d.credential.as_ref().and_then(|k| cred_ids.get(k).cloned()),
                jump_chain: hops(&d.jump_chain),
                keepalive_s: d.keepalive_s,
                ..ConnectionDefaults::default()
            },
        },
    )?;
    let mut folders_created = 1usize;

    // Sous-dossiers (parents d'abord).
    let mut folder_ids: HashMap<Vec<String>, String> = HashMap::new();
    folder_ids.insert(Vec::new(), root_id.clone());
    let mut paths: Vec<&Vec<String>> = plan.folders.iter().collect();
    paths.sort_by_key(|p| p.len());
    for (order, path) in paths.into_iter().enumerate() {
        ensure_folder(
            store,
            path,
            &mut folder_ids,
            &mut folders_created,
            &mut warnings,
            i32::try_from(order).unwrap_or(0),
        );
    }

    // Connexions.
    let mut used: HashMap<String, HashSet<String>> = HashMap::new();
    let mut imported = 0usize;
    for pc in plan.connections.iter().take(MAX_CONNECTIONS) {
        let host = clean(&pc.host);
        if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c == '/') {
            skipped += 1;
            push_w(&mut warnings, "invalid_host", &[("name", &pc.name)]);
            continue;
        }
        let folder_id = folder_ids
            .get(&pc.folder_path)
            .cloned()
            .unwrap_or_else(|| root_id.clone());
        let base = if clean(&pc.name).is_empty() {
            host.clone()
        } else {
            clean(&pc.name)
        };
        let name = unique_name(&base, used.entry(folder_id.clone()).or_default());
        let now = crate::crypto::now_rfc3339();
        let conn = Connection {
            id: String::new(),
            folder_id,
            name: name.clone(),
            protocol: "ssh".into(),
            host,
            port: pc.port,
            user: pc.user.as_deref().map(clean).filter(|u| !u.is_empty()),
            credential_id: pc
                .credential
                .as_ref()
                .and_then(|k| cred_ids.get(k).cloned()),
            jump_chain: pc.jump_chain.as_deref().map(hops),
            clipboard_policy: None,
            keepalive_s: pc.keepalive_s,
            encoding: None,
            color: None,
            icon: None,
            tags: Vec::new(),
            favorite: false,
            tunnels: pc
                .tunnels
                .iter()
                .map(|t| TunnelSpec {
                    id: Uuid::new_v4().to_string(),
                    kind: t.kind.clone(),
                    bind_addr: t.bind_addr.clone(),
                    bind_port: t.bind_port,
                    dest_host: t.dest_host.clone(),
                    dest_port: t.dest_port,
                    auto_start: false,
                })
                .collect(),
            startup_snippet_id: None,
            session_logging: false,
            sftp: SftpOptions::default(),
            notes: String::new(),
            created_at: now.clone(),
            updated_at: now,
            last_used_at: None,
        };
        match manage::upsert_connection(store, conn) {
            Ok(()) => imported += 1,
            Err(_) => {
                skipped += 1;
                push_w(&mut warnings, "connection_rejected", &[("name", &name)]);
            }
        }
    }
    if plan.connections.len() > MAX_CONNECTIONS {
        skipped += plan.connections.len() - MAX_CONNECTIONS;
        push_w(&mut warnings, "too_many_connections", &[]);
    }

    manage::validate_store(store)?;
    store.touch();
    Ok((
        ImportReport {
            source: plan.source,
            root_folder: root_name,
            imported,
            folders: folders_created,
            credentials: plan.credentials.len(),
            skipped,
            secrets_to_reenter: reenter,
            warnings,
        },
        secrets,
    ))
}

fn push_w(warnings: &mut Vec<ImportWarning>, code: &str, params: &[(&str, &str)]) {
    if warnings.len() < MAX_WARNINGS {
        warnings.push(ImportWarning::new(code, params));
    }
}

fn ensure_folder(
    store: &mut Store,
    path: &[String],
    ids: &mut HashMap<Vec<String>, String>,
    created: &mut usize,
    warnings: &mut Vec<ImportWarning>,
    order: i32,
) -> String {
    if let Some(id) = ids.get(path) {
        return id.clone();
    }
    let parent = ensure_folder(
        store,
        &path[..path.len() - 1],
        ids,
        created,
        warnings,
        order,
    );
    let name = clean(&path[path.len() - 1]);
    let name = if name.is_empty() {
        "?".to_string()
    } else {
        name
    };
    let id = Uuid::new_v4().to_string();
    let folder = Folder {
        id: id.clone(),
        parent_id: Some(parent.clone()),
        name: name.clone(),
        color: None,
        icon: None,
        sort_order: order,
        tags: Vec::new(),
        defaults: ConnectionDefaults::default(),
    };
    let result = match manage::upsert_folder(store, folder) {
        Ok(()) => {
            *created += 1;
            id
        }
        Err(_) => {
            push_w(warnings, "folder_rejected", &[("name", &name)]);
            parent
        }
    };
    ids.insert(path.to_vec(), result.clone());
    result
}

/// Applique le plan au coffre (copie de travail → remplacement atomique en mémoire,
/// puis chiffrement des clés). L'appelant fait `vault.save()` et l'audit.
pub fn apply(
    vault: &mut Vault,
    plan: &ImportPlan,
    keys: &HashMap<String, KeyMaterial>,
) -> Result<ImportReport> {
    let mut work = vault.store.clone();
    let (report, secrets) = apply_to_store(&mut work, plan, keys)?;
    let backup = std::mem::replace(&mut vault.store, work);
    for s in &secrets {
        if let Err(e) = vault.set_secret(&s.credential_id, s.field, &s.value) {
            vault.store = backup;
            return Err(e);
        }
    }
    Ok(report)
}

// ------------------------------------------------------------ aperçu (dry-run)

#[derive(Clone, Debug, Serialize)]
pub struct SummaryConnection {
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub folder: String,
    pub jumps: usize,
    pub tunnels: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct SummaryKey {
    /// Chemin du fichier de clé (non secret).
    pub path: String,
    /// `ok` | `encrypted` | `unreadable`
    pub status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportSummary {
    pub source: ImportSource,
    pub root_folder: String,
    pub folders: Vec<String>,
    pub connections: Vec<SummaryConnection>,
    pub credentials: usize,
    pub keys: Vec<SummaryKey>,
    pub secrets_to_reenter: usize,
    pub skipped: usize,
    pub warnings: Vec<ImportWarning>,
}

/// Résumé sans effet de bord ni secret : ce que l'import créerait.
pub fn summarize(plan: &ImportPlan, keys: &HashMap<String, KeyMaterial>) -> ImportSummary {
    let mut key_files: Vec<SummaryKey> = Vec::new();
    for pc in &plan.credentials {
        if let Some(kf) = &pc.key_file
            && !key_files.iter().any(|k| &k.path == kf)
        {
            let status = match keys.get(kf) {
                Some(k) if k.encrypted => "encrypted",
                Some(_) => "ok",
                None => "unreadable",
            };
            key_files.push(SummaryKey {
                path: kf.clone(),
                status: status.into(),
            });
        }
    }
    ImportSummary {
        source: plan.source,
        root_folder: plan.source.root_folder_name().to_string(),
        folders: plan.folders.iter().map(|p| p.join("/")).collect(),
        connections: plan
            .connections
            .iter()
            .take(MAX_CONNECTIONS)
            .map(|c| SummaryConnection {
                name: c.name.clone(),
                host: c.host.clone(),
                port: c.port,
                user: c.user.clone(),
                folder: c.folder_path.join("/"),
                jumps: c.jump_chain.as_ref().map_or(0, Vec::len),
                tunnels: c.tunnels.len(),
            })
            .collect(),
        credentials: plan.credentials.len(),
        keys: key_files,
        secrets_to_reenter: plan
            .credentials
            .iter()
            .filter(|pc| !reentry_fields(pc, keys).is_empty())
            .count(),
        skipped: plan.skipped,
        warnings: plan.warnings.clone(),
    }
}
