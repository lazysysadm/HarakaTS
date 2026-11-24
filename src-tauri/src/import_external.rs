//! Imports depuis d'autres outils (M7) : `~/.ssh/config`, Remmina, MobaXterm.
//!
//! Le fichier est choisi par dialogue natif (rfd), lu et analysé ici ; les clés
//! privées référencées sont lues puis gardées **côté backend** (plan stocké sous
//! un identifiant d'aperçu). L'UI ne reçoit qu'un résumé sans secret. Rien n'est
//! écrit avant `import_external_apply`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use haraka_core::import::ssh_config::{self, FsIncludes};
use haraka_core::import::{
    self, ImportPlan, ImportReport, ImportSource, ImportSummary, ImportWarning, KeyMaterial,
    decode_text,
};
use secrecy::SecretString;
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

const LOCKED: &str = "coffre verrouillé";
const MAX_FILES: usize = 500;
const MAX_KEY_BYTES: u64 = 65_536;
const MAX_PREVIEWS: usize = 8;

struct Stored {
    plan: ImportPlan,
    keys: HashMap<String, KeyMaterial>,
}

fn store() -> &'static Mutex<HashMap<String, Stored>> {
    static S: OnceLock<Mutex<HashMap<String, Stored>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Serialize)]
pub struct ExternalPreviewDto {
    pub preview_id: String,
    #[serde(flatten)]
    pub summary: ImportSummary,
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

fn read_file(path: &Path) -> Result<(String, Vec<ImportWarning>), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("fichier illisible : {e}"))?;
    if !meta.is_file() {
        return Err("fichier illisible : pas un fichier régulier".into());
    }
    if meta.len() > import::MAX_INPUT_BYTES as u64 * 2 {
        return Err("fichier illisible : trop volumineux".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("fichier illisible : {e}"))?;
    Ok(decode_text(&bytes))
}

/// Lit une clé privée référencée ; `Err(code)` = raison courte pour l'avertissement.
fn read_key(path: &str) -> Result<KeyMaterial, &'static str> {
    let meta = std::fs::metadata(path).map_err(|_| "missing")?;
    if !meta.is_file() {
        return Err("missing");
    }
    if meta.len() > MAX_KEY_BYTES {
        return Err("too_big");
    }
    let bytes = std::fs::read(path).map_err(|_| "unreadable")?;
    let text = String::from_utf8(bytes).map_err(|_| "not_a_key")?;
    let t = text.trim();
    if t.contains("PUBLIC KEY-----") || t.starts_with("ssh-") || t.starts_with("ecdsa-") {
        return Err("public_key");
    }
    if !t.contains("-----BEGIN") || !t.contains("PRIVATE KEY-----") {
        return Err("not_a_key");
    }
    let info = haraka_ssh::inspect_private_key(&text).map_err(|_| "invalid")?;
    Ok(KeyMaterial {
        pem: SecretString::from(text),
        encrypted: info.encrypted,
    })
}

fn load_keys(plan: &mut ImportPlan) -> HashMap<String, KeyMaterial> {
    let mut keys = HashMap::new();
    let paths: Vec<String> = plan
        .credentials
        .iter()
        .filter_map(|c| c.key_file.clone())
        .collect();
    for p in paths {
        if keys.contains_key(&p) {
            continue;
        }
        match read_key(&p) {
            Ok(k) => {
                keys.insert(p, k);
            }
            Err(reason) => plan.warn("key_unreadable", &[("path", &p), ("reason", reason)]),
        }
    }
    keys
}

fn remmina_default_dir() -> Option<PathBuf> {
    let h = home();
    if h.is_empty() {
        return None;
    }
    [".local/share/remmina", ".config/remmina", ".remmina"]
        .iter()
        .map(|d| PathBuf::from(&h).join(d))
        .find(|p| p.is_dir())
}

/// Boîte de dialogue : `(chemins choisis)` ou `None` si annulé.
async fn pick(source: ImportSource, directory: bool) -> CmdResult<Vec<PathBuf>> {
    let h = home();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut d = rfd::FileDialog::new();
        match source {
            ImportSource::SshConfig => {
                d = d.set_title("Importer ~/.ssh/config");
                if !h.is_empty() {
                    d = d.set_directory(format!("{h}/.ssh"));
                }
                d.pick_file().map(|p| vec![p])
            }
            ImportSource::Remmina => {
                d = d.set_title("Importer des profils Remmina");
                if let Some(dir) = remmina_default_dir() {
                    d = d.set_directory(dir);
                }
                if directory {
                    d.pick_folder().map(|p| vec![p])
                } else {
                    d.add_filter("Remmina", &["remmina"]).pick_files()
                }
            }
            ImportSource::Mobaxterm => d
                .set_title("Importer des sessions MobaXterm")
                .add_filter("MobaXterm", &["mxtsessions"])
                .pick_file()
                .map(|p| vec![p]),
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    picked.ok_or_else(|| "Import annulé.".to_string())
}

fn build_plan(source: ImportSource, paths: &[PathBuf]) -> CmdResult<ImportPlan> {
    let h = home();
    let mut extra: Vec<ImportWarning> = Vec::new();
    let mut plan = match source {
        ImportSource::SshConfig => {
            let path = paths.first().ok_or("aucun fichier sélectionné")?;
            let (text, w) = read_file(path)?;
            extra = w;
            let dir = path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let mut prov = FsIncludes {
                ssh_dir: dir.clone(),
            };
            ssh_config::parse(&text, &h, &dir, &mut prov)
        }
        ImportSource::Remmina => {
            let mut files: Vec<PathBuf> = Vec::new();
            for p in paths {
                if p.is_dir() {
                    let mut found: Vec<PathBuf> = std::fs::read_dir(p)
                        .map_err(|e| format!("fichier illisible : {e}"))?
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| {
                            p.extension()
                                .is_some_and(|x| x.eq_ignore_ascii_case("remmina"))
                        })
                        .collect();
                    found.sort();
                    files.extend(found);
                } else {
                    files.push(p.clone());
                }
            }
            if files.is_empty() {
                return Err("aucun fichier .remmina trouvé dans ce dossier".into());
            }
            if files.len() > MAX_FILES {
                return Err("trop de fichiers sélectionnés (500 maximum)".into());
            }
            let mut inputs = Vec::new();
            for f in &files {
                let name = f
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                match read_file(f) {
                    Ok((text, w)) => {
                        extra.extend(w);
                        inputs.push((name, text));
                    }
                    Err(_) => extra.push(ImportWarning::new("file_unreadable", &[("file", &name)])),
                }
            }
            import::remmina::parse(&inputs, &h)
        }
        ImportSource::Mobaxterm => {
            let path = paths.first().ok_or("aucun fichier sélectionné")?;
            let (text, w) = read_file(path)?;
            extra = w;
            import::mobaxterm::parse(&text)
        }
    };
    for w in extra {
        plan.push_warning(w);
    }
    Ok(plan)
}

/// Aperçu (dry-run) : dialogue natif, lecture, analyse. Le plan et les clés
/// restent côté backend ; seul un résumé sans secret est renvoyé.
#[tauri::command]
pub async fn import_external_preview(
    state: State<'_, AppState>,
    source: String,
    directory: Option<bool>,
) -> CmdResult<ExternalPreviewDto> {
    let source =
        ImportSource::parse(&source).map_err(|_| "source d'import inconnue".to_string())?;
    {
        let inner = state.inner.lock().await;
        if inner.vault.is_none() {
            return Err(LOCKED.into());
        }
    }
    let paths = pick(source, directory.unwrap_or(false)).await?;
    let mut plan = tauri::async_runtime::spawn_blocking(move || build_plan(source, &paths))
        .await
        .map_err(|e| e.to_string())??;
    let (plan, keys) = tauri::async_runtime::spawn_blocking(move || {
        let keys = load_keys(&mut plan);
        (plan, keys)
    })
    .await
    .map_err(|e| e.to_string())?;
    let summary = import::summarize(&plan, &keys);
    let preview_id = uuid::Uuid::new_v4().to_string();
    let mut map = store().lock().map_err(|_| "état interne indisponible")?;
    if map.len() >= MAX_PREVIEWS {
        map.clear();
    }
    map.insert(preview_id.clone(), Stored { plan, keys });
    Ok(ExternalPreviewDto {
        preview_id,
        summary,
    })
}

/// Applique un aperçu : crée dossier/credentials/connexions, enregistre le
/// coffre, journalise (source + compteurs uniquement).
#[tauri::command]
pub async fn import_external_apply(
    state: State<'_, AppState>,
    preview_id: String,
) -> CmdResult<ImportReport> {
    let stored = store()
        .lock()
        .map_err(|_| "état interne indisponible")?
        .remove(&preview_id)
        .ok_or("aperçu expiré — relancez l'import")?;
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    let report = import::apply(vault, &stored.plan, &stored.keys).map_err(|e| e.to_string())?;
    vault.save().map_err(|e| e.to_string())?;
    haraka_core::audit::append(
        vault.data_dir(),
        "import_external",
        Some(report.source.as_str()),
        Some(&format!(
            "imported={}, folders={}, credentials={}, skipped={}, reenter={}",
            report.imported,
            report.folders,
            report.credentials,
            report.skipped,
            report.secrets_to_reenter
        )),
    )
    .map_err(|e| e.to_string())?;
    Ok(report)
}
