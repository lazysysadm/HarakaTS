//! Commandes Tauri M1 : coffre, réglages, export/import, audit.
//! Aucun secret ne sort du backend (sauf action explicite à partir de M2) ;
//! les mots de passe saisis transitent frontend → backend et sont immédiatement
//! enveloppés dans `secrecy`.

use std::collections::HashMap;

use haraka_core::export::{self, ExportScope, Strategy};
use haraka_core::format::{Envelope, VaultHeader};
use haraka_core::model::Settings;
use haraka_core::store::{self, DataDir};
use haraka_core::vault::{CreateOptions, SecretField, Vault};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::state::{AppState, Inner};

type CmdResult<T> = Result<T, String>;

fn err(e: haraka_core::Error) -> String {
    e.to_string()
}

pub(crate) const LOCKED: &str = "coffre verrouillé";

#[derive(Clone, Serialize)]
pub struct VaultStatus {
    pub state: String,
    pub keyring_available: bool,
    pub keyring_cache_enabled: bool,
    pub autolock_minutes: u32,
    pub data_dir: String,
    pub vault_id: Option<String>,
    pub app_version: String,
}

#[derive(Clone, Serialize)]
pub struct ExportResult {
    pub path: String,
    pub counts: haraka_core::format::Counts,
}

#[derive(Clone, Serialize)]
pub struct PreviewDto {
    pub preview_id: String,
    pub counts: haraka_core::format::Counts,
    pub conflicts: Vec<export::Conflict>,
    pub includes_secrets: bool,
    pub source_vault_id: String,
}

#[derive(Deserialize)]
pub struct StrategyOverride {
    pub key: String,
    pub strategy: String,
}

#[derive(Clone, Serialize)]
pub struct BackupDto {
    pub index: u32,
    pub modified_epoch: Option<u64>,
    pub valid: bool,
}

fn locked_vault_id(dd: &DataDir) -> Option<String> {
    let bytes = std::fs::read(dd.vault_file()).ok()?;
    let envelope = Envelope::decode(&bytes).ok()?;
    let header: VaultHeader = envelope.header_json().ok()?;
    Some(header.vault_id)
}

fn status_of(dd: &DataDir, inner: &Inner) -> VaultStatus {
    let state = if !dd.vault_exists() {
        "uninitialized"
    } else if inner.vault.is_some() {
        "unlocked"
    } else {
        "locked"
    };
    let (keyring_available, keyring_cache_enabled, autolock_minutes, vault_id) = match &inner.vault
    {
        Some(v) => (
            v.keyring_available(),
            v.store.settings.keyring_cache,
            v.store.settings.autolock_minutes,
            Some(v.vault_id().to_string()),
        ),
        None => {
            let vid = locked_vault_id(dd);
            let available = vid
                .as_deref()
                .map(haraka_core::keyring_cache::available)
                .unwrap_or(false);
            (available, false, inner.autolock.delay_minutes(), vid)
        }
    };
    VaultStatus {
        state: state.to_string(),
        keyring_available,
        keyring_cache_enabled,
        autolock_minutes,
        data_dir: dd.root().display().to_string(),
        vault_id,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn emit(app: &AppHandle, status: &VaultStatus) {
    // Coffre déverrouillé : les terminaux masqués au verrouillage (sessions conservées) reviennent.
    #[cfg(target_os = "linux")]
    if status.state == "unlocked" {
        crate::vte::set_all_visible(true);
    }
    let _ = app.emit("vault://state", status);
}

#[tauri::command]
pub async fn vault_status(state: State<'_, AppState>) -> CmdResult<VaultStatus> {
    let inner = state.inner.lock().await;
    Ok(status_of(&state.data_dir, &inner))
}

#[tauri::command]
pub async fn vault_create(
    state: State<'_, AppState>,
    app: AppHandle,
    master: String,
    confirm: String,
    keyring_cache: bool,
) -> CmdResult<VaultStatus> {
    let master = SecretString::from(master);
    let confirm = SecretString::from(confirm);
    if master.expose_secret() != confirm.expose_secret() {
        return Err("Les deux saisies du mot de passe maître diffèrent.".into());
    }
    if master.expose_secret().len() < 8 {
        return Err("Mot de passe maître trop court (8 caractères minimum).".into());
    }
    let dd = state.data_dir.clone();
    let opts = CreateOptions {
        keyring_cache,
        kdf: None, // calibration 0,5–1 s sur la machine
    };
    let vault = tauri::async_runtime::spawn_blocking(move || Vault::create(dd, &master, opts))
        .await
        .map_err(|e| e.to_string())?
        .map_err(err)?;
    let mut inner = state.inner.lock().await;
    inner
        .autolock
        .set_delay_minutes(vault.store.settings.autolock_minutes);
    inner.vault = Some(vault);
    let status = status_of(&state.data_dir, &inner);
    emit(&app, &status);
    Ok(status)
}

#[tauri::command]
pub async fn vault_unlock(
    state: State<'_, AppState>,
    app: AppHandle,
    master: Option<String>,
    use_keyring: bool,
) -> CmdResult<VaultStatus> {
    let dd = state.data_dir.clone();
    let master = master.map(SecretString::from);
    let vault = tauri::async_runtime::spawn_blocking(move || {
        if use_keyring {
            match Vault::unlock_with_keyring(dd.clone()) {
                Ok(v) => Ok(v),
                Err(e) => match master {
                    Some(m) => Vault::unlock(dd, &m),
                    None => Err(e),
                },
            }
        } else {
            let m = master.ok_or_else(|| {
                haraka_core::Error::InvalidParams("mot de passe maître requis".into())
            })?;
            Vault::unlock(dd, &m)
        }
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(err)?;
    let mut inner = state.inner.lock().await;
    inner
        .autolock
        .set_delay_minutes(vault.store.settings.autolock_minutes);
    // Rétention des journaux de session : purge à chaque déverrouillage.
    let removed = haraka_core::sessionlog::purge_older_than(
        &state.data_dir.logs_dir(),
        vault.store.settings.session_log_retention_days,
    );
    if removed > 0 {
        let _ = haraka_core::audit::append(
            &state.data_dir,
            "session_logs_purge",
            None,
            Some(&format!("auto removed={removed}")),
        );
    }
    inner.vault = Some(vault);
    let status = status_of(&state.data_dir, &inner);
    emit(&app, &status);
    Ok(status)
}

#[tauri::command]
pub async fn vault_lock(state: State<'_, AppState>, app: AppHandle) -> CmdResult<VaultStatus> {
    let mut inner = state.inner.lock().await;
    let mut disconnect = true;
    if let Some(vault) = inner.vault.take() {
        disconnect = vault.store.settings.disconnect_on_lock;
        vault.lock();
    }
    inner.previews.clear();
    drop(inner);
    if disconnect {
        crate::sessions::close_all_sessions(&state).await;
    } else {
        crate::sessions::lock_keep_sessions(&state).await;
    }
    let inner = state.inner.lock().await;
    let status = status_of(&state.data_dir, &inner);
    emit(&app, &status);
    Ok(status)
}

#[tauri::command]
pub async fn vault_change_master(
    state: State<'_, AppState>,
    old: String,
    new: String,
    confirm: String,
) -> CmdResult<()> {
    let old = SecretString::from(old);
    let new = SecretString::from(new);
    let confirm = SecretString::from(confirm);
    if new.expose_secret() != confirm.expose_secret() {
        return Err("Les deux saisies du nouveau mot de passe diffèrent.".into());
    }
    if new.expose_secret().len() < 8 {
        return Err("Nouveau mot de passe trop court (8 caractères minimum).".into());
    }
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    vault.change_master(&old, &new).map_err(err)?;
    Ok(())
}

#[tauri::command]
pub async fn vault_keyring_set_enabled(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> CmdResult<VaultStatus> {
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    vault.keyring_set_enabled(enabled).map_err(err)?;
    let status = status_of(&state.data_dir, &inner);
    emit(&app, &status);
    Ok(status)
}

#[tauri::command]
pub async fn vault_activity(state: State<'_, AppState>) -> CmdResult<()> {
    let mut inner = state.inner.lock().await;
    inner.autolock.activity();
    Ok(())
}

#[tauri::command]
pub async fn vault_list_backups(state: State<'_, AppState>) -> CmdResult<Vec<BackupDto>> {
    Ok(store::list_backups(&state.data_dir)
        .into_iter()
        .map(|(index, modified, valid)| BackupDto {
            index,
            modified_epoch: modified
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
            valid,
        })
        .collect())
}

/// Le fichier de coffre est-il structurellement lisible ? (faux : proposer la restauration)
#[tauri::command]
pub async fn vault_integrity(state: State<'_, AppState>) -> CmdResult<bool> {
    let path = state.data_dir.vault_file();
    Ok(!path.exists() || store::envelope_valid(&path))
}

#[tauri::command]
pub async fn vault_restore_backup(
    state: State<'_, AppState>,
    app: AppHandle,
    index: u32,
) -> CmdResult<VaultStatus> {
    let inner = state.inner.lock().await;
    if inner.vault.is_some() {
        return Err("verrouillez le coffre avant de restaurer une sauvegarde".into());
    }
    store::restore_backup(&state.data_dir, index).map_err(err)?;
    haraka_core::audit::append(
        &state.data_dir,
        "backup_restore",
        None,
        Some(&format!("index={index}")),
    )
    .map_err(err)?;
    let status = status_of(&state.data_dir, &inner);
    emit(&app, &status);
    Ok(status)
}

#[tauri::command]
pub async fn settings_get(state: State<'_, AppState>) -> CmdResult<Settings> {
    let inner = state.inner.lock().await;
    let vault = inner.vault.as_ref().ok_or(LOCKED)?;
    Ok(vault.store.settings.clone())
}

#[tauri::command]
pub async fn settings_set(
    state: State<'_, AppState>,
    patch: serde_json::Value,
) -> CmdResult<Settings> {
    let mut inner = state.inner.lock().await;
    let minutes = {
        let vault = inner.vault.as_mut().ok_or(LOCKED)?;
        let mut current = serde_json::to_value(&vault.store.settings).map_err(|e| e.to_string())?;
        let (Some(base), Some(patch)) = (current.as_object_mut(), patch.as_object()) else {
            return Err("patch de réglages invalide".into());
        };
        for (key, value) in patch {
            base.insert(key.clone(), value.clone());
        }
        let updated: Settings =
            serde_json::from_value(current).map_err(|e| format!("réglages invalides : {e}"))?;
        vault.store.settings = updated;
        vault.save().map_err(err)?;
        vault.store.settings.autolock_minutes
    };
    inner.autolock.set_delay_minutes(minutes);
    let vault = inner.vault.as_ref().ok_or(LOCKED)?;
    Ok(vault.store.settings.clone())
}

#[tauri::command]
pub async fn export_bundle(
    state: State<'_, AppState>,
    scope: String,
    folder_id: Option<String>,
    passphrase: String,
    confirm: String,
) -> CmdResult<ExportResult> {
    let passphrase = SecretString::from(passphrase);
    let confirm = SecretString::from(confirm);
    if passphrase.expose_secret() != confirm.expose_secret() {
        return Err("Les deux saisies de la passphrase d'export diffèrent.".into());
    }
    if passphrase.expose_secret().len() < 8 {
        return Err("Passphrase d'export trop courte (8 caractères minimum).".into());
    }
    let scope = ExportScope::parse(&scope, folder_id).map_err(err)?;
    let path = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Exporter le bundle HarakaTS")
            .set_file_name("harakats-export.hrkx")
            .add_filter("Bundle HarakaTS", &["hrkx"])
            .save_file()
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or("Export annulé.")?;
    let inner = state.inner.lock().await;
    let vault = inner.vault.as_ref().ok_or(LOCKED)?;
    let counts = export::export_bundle(vault, &scope, &passphrase, None, &path).map_err(err)?;
    Ok(ExportResult {
        path: path.display().to_string(),
        counts,
    })
}

#[tauri::command]
pub async fn import_pick_and_preview(
    state: State<'_, AppState>,
    passphrase: String,
) -> CmdResult<PreviewDto> {
    let path = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Importer un bundle HarakaTS")
            .add_filter("Bundle HarakaTS", &["hrkx"])
            .pick_file()
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or("Import annulé.")?;
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_ref().ok_or(LOCKED)?;
    let preview =
        export::preview(&path, &SecretString::from(passphrase), &vault.store).map_err(err)?;
    let dto = PreviewDto {
        preview_id: preview.id.clone(),
        counts: preview.counts(),
        conflicts: preview.conflicts.clone(),
        includes_secrets: preview.header.content.includes_secrets,
        source_vault_id: preview.header.source_vault_id.clone(),
    };
    inner.previews.insert(preview.id.clone(), preview);
    Ok(dto)
}

#[tauri::command]
pub async fn import_apply(
    state: State<'_, AppState>,
    preview_id: String,
    default_strategy: String,
    overrides: Vec<StrategyOverride>,
) -> CmdResult<haraka_core::format::Counts> {
    let default = Strategy::parse(&default_strategy).map_err(err)?;
    let mut map = HashMap::new();
    for item in overrides {
        map.insert(item.key, Strategy::parse(&item.strategy).map_err(err)?);
    }
    let mut inner = state.inner.lock().await;
    let preview = inner
        .previews
        .remove(&preview_id)
        .ok_or("aperçu expiré — relancez l'import")?;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    export::apply(vault, preview, default, &map).map_err(err)
}

#[tauri::command]
pub async fn audit_log_list(
    state: State<'_, AppState>,
    limit: usize,
) -> CmdResult<Vec<serde_json::Value>> {
    haraka_core::audit::list(&state.data_dir, limit.min(1000)).map_err(err)
}

// ---------------------------------------------------------------- M2 : arbre

#[derive(Clone, Serialize)]
pub struct CredentialMeta {
    pub id: String,
    pub name: String,
    pub username: String,
    pub has_password: bool,
    pub has_private_key: bool,
    pub has_passphrase: bool,
    pub needs_reentry: Vec<String>,
    pub ref_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Serialize)]
pub struct MacroMeta {
    pub id: String,
    pub name: String,
    pub confirm_before_send: bool,
}

#[derive(Clone, Serialize)]
pub struct TreeDto {
    pub folders: Vec<haraka_core::model::Folder>,
    pub connections: Vec<haraka_core::model::Connection>,
    pub credentials: Vec<CredentialMeta>,
    pub macros: Vec<MacroMeta>,
    pub recents: Vec<haraka_core::model::Recent>,
    pub ui_state: serde_json::Value,
}

fn tree_of(store: &haraka_core::Store) -> TreeDto {
    TreeDto {
        folders: store.folders.clone(),
        connections: store.connections.clone(),
        credentials: store
            .credentials
            .iter()
            .map(|c| CredentialMeta {
                id: c.id.clone(),
                name: c.name.clone(),
                username: c.username.clone(),
                has_password: c.password.is_some(),
                has_private_key: c.private_key.is_some(),
                has_passphrase: c.passphrase.is_some(),
                needs_reentry: c.needs_reentry.clone(),
                ref_count: haraka_core::manage::credential_refs(store, &c.id).len(),
                created_at: c.created_at.clone(),
                updated_at: c.updated_at.clone(),
            })
            .collect(),
        macros: store
            .macros
            .iter()
            .map(|m| MacroMeta {
                id: m.id.clone(),
                name: m.name.clone(),
                confirm_before_send: m.confirm_before_send,
            })
            .collect(),
        recents: store.recents.clone(),
        ui_state: store.ui_state.clone(),
    }
}

fn save_tree(vault: &mut Vault) -> CmdResult<TreeDto> {
    vault.save().map_err(err)?;
    Ok(tree_of(&vault.store))
}

#[tauri::command]
pub async fn tree_get(state: State<'_, AppState>) -> CmdResult<TreeDto> {
    let inner = state.inner.lock().await;
    let vault = inner.vault.as_ref().ok_or(LOCKED)?;
    Ok(tree_of(&vault.store))
}

#[tauri::command]
pub async fn folder_upsert(
    state: State<'_, AppState>,
    folder: haraka_core::model::Folder,
) -> CmdResult<TreeDto> {
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    haraka_core::manage::upsert_folder(&mut vault.store, folder).map_err(err)?;
    save_tree(vault)
}

#[tauri::command]
pub async fn folder_delete(
    state: State<'_, AppState>,
    id: String,
    cascade: bool,
) -> CmdResult<TreeDto> {
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    haraka_core::manage::delete_folder(&mut vault.store, &id, cascade).map_err(err)?;
    save_tree(vault)
}

#[tauri::command]
pub async fn folder_move(
    state: State<'_, AppState>,
    id: String,
    new_parent: Option<String>,
    sort_order: Option<i32>,
) -> CmdResult<TreeDto> {
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    haraka_core::manage::move_folder(&mut vault.store, &id, new_parent.as_deref(), sort_order)
        .map_err(err)?;
    save_tree(vault)
}

#[tauri::command]
pub async fn connection_upsert(
    state: State<'_, AppState>,
    connection: haraka_core::model::Connection,
) -> CmdResult<TreeDto> {
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    haraka_core::manage::upsert_connection(&mut vault.store, connection).map_err(err)?;
    save_tree(vault)
}

#[tauri::command]
pub async fn connection_delete(state: State<'_, AppState>, id: String) -> CmdResult<TreeDto> {
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    haraka_core::manage::delete_connection(&mut vault.store, &id).map_err(err)?;
    save_tree(vault)
}

#[tauri::command]
pub async fn connection_duplicate(state: State<'_, AppState>, id: String) -> CmdResult<TreeDto> {
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    haraka_core::manage::duplicate_connection(&mut vault.store, &id).map_err(err)?;
    save_tree(vault)
}

#[tauri::command]
pub async fn connection_effective(
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<haraka_core::EffectiveConfig> {
    let inner = state.inner.lock().await;
    let vault = inner.vault.as_ref().ok_or(LOCKED)?;
    let conn = vault
        .store
        .connections
        .iter()
        .find(|c| c.id == id)
        .ok_or("connexion introuvable")?;
    Ok(haraka_core::effective::resolve(&vault.store, conn))
}

// ------------------------------------------------- M2 : credentials partagés

/// Métadonnées non secrètes d'une clé importée depuis un fichier.
#[derive(Clone, Serialize)]
pub struct KeyImportDto {
    pub pending_id: String,
    pub info: haraka_ssh::KeyInfo,
    pub file_name: String,
    pub public_key: Option<String>,
    pub permissions_warning: Option<String>,
}

/// Import d'une clé privée depuis un fichier (rfd natif, lecture backend).
/// Le contenu reste côté backend (`pending_keys`) jusqu'à l'enregistrement ;
/// seules les métadonnées (type, empreinte, commentaire) vont à l'UI.
#[tauri::command]
pub async fn credential_import_key_file(state: State<'_, AppState>) -> CmdResult<KeyImportDto> {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new().set_title("Importer une clé privée");
        if !home.is_empty() {
            dialog = dialog.set_directory(format!("{home}/.ssh"));
        }
        dialog.pick_file()
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or("aucun fichier sélectionné")?;

    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > 65_536 {
        return Err("fichier trop volumineux (max 64 Ko) — ce n'est pas une clé privée".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes)
        .map_err(|_| "fichier binaire : ce n'est pas une clé privée".to_string())?;
    let trimmed = text.trim();
    if trimmed.starts_with("ssh-ed25519 ")
        || trimmed.starts_with("ssh-rsa ")
        || trimmed.starts_with("ssh-dss ")
        || trimmed.starts_with("ecdsa-sha2-")
        || trimmed.contains("PUBLIC KEY-----")
    {
        return Err("c'est une clé PUBLIQUE — importez la clé privée (fichier sans .pub)".into());
    }
    if !trimmed.contains("-----BEGIN") || !trimmed.contains("PRIVATE KEY-----") {
        return Err(
            "ce fichier n'est pas une clé privée (en-tête -----BEGIN … PRIVATE KEY----- absent)"
                .into(),
        );
    }
    let info = haraka_ssh::inspect_private_key(&text)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let public_key = std::fs::read_to_string(format!("{}.pub", path.display()))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    #[cfg(unix)]
    let permissions_warning = {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            Some(format!(
                "permissions trop larges sur le fichier source : {mode:o} (0600 recommandé)"
            ))
        } else {
            None
        }
    };
    #[cfg(not(unix))]
    let permissions_warning: Option<String> = None;

    let pending_id = uuid::Uuid::new_v4().to_string();
    {
        let mut inner = state.inner.lock().await;
        inner.pending_keys.insert(
            pending_id.clone(),
            crate::state::PendingKey {
                pem: SecretString::from(text),
                info: info.clone(),
                file_name: file_name.clone(),
            },
        );
    }
    let _ = haraka_core::audit::append(
        &state.data_dir,
        "credential_key_import",
        Some(&file_name),
        Some(&info.fingerprint),
    );
    Ok(KeyImportDto {
        pending_id,
        info,
        file_name,
        public_key,
        permissions_warning,
    })
}

/// `None` = inchangé, `Some(None)` = effacer, `Some(Some(v))` = définir.
/// `pending_key_id` : clé importée depuis un fichier (le PEM vient du
/// backend, jamais de l'UI) ; la passphrase est vérifiée avant écriture.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn credential_upsert(
    state: State<'_, AppState>,
    id: Option<String>,
    name: String,
    username: String,
    password: Option<Option<String>>,
    private_key: Option<Option<String>>,
    passphrase: Option<Option<String>>,
    pending_key_id: Option<String>,
) -> CmdResult<TreeDto> {
    let mut inner = state.inner.lock().await;
    let pending = match pending_key_id {
        Some(pid) => inner.pending_keys.remove(&pid),
        None => None,
    };
    let pending_file = pending.as_ref().map(|pk| pk.file_name.clone());
    let private_key = match &pending {
        Some(pk) => {
            if pk.info.encrypted {
                let pp = match &passphrase {
                    Some(Some(v)) => v.as_str(),
                    _ => "",
                };
                if pp.is_empty() {
                    return Err("passphrase requise : cette clé est chiffrée".into());
                }
                haraka_ssh::check_key_passphrase(pk.pem.expose_secret(), pp)?;
            }
            Some(Some(pk.pem.expose_secret().to_string()))
        }
        None => private_key,
    };
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    let cred_id = haraka_core::manage::upsert_credential_meta(
        &mut vault.store,
        id.as_deref(),
        &name,
        &username,
    )
    .map_err(err)?;
    for (field, value) in [
        (SecretField::Password, password),
        (SecretField::PrivateKey, private_key),
        (SecretField::Passphrase, passphrase),
    ] {
        match value {
            Some(Some(v)) => {
                vault
                    .set_secret(&cred_id, field, &SecretString::from(v))
                    .map_err(err)?;
            }
            Some(None) => vault.clear_secret(&cred_id, field).map_err(err)?,
            None => {}
        }
    }
    haraka_core::audit::append(
        vault.data_dir(),
        "credential_upsert",
        Some(&cred_id),
        pending_file.as_deref(),
    )
    .map_err(err)?;
    save_tree(vault)
}

#[tauri::command]
pub async fn credential_delete(
    state: State<'_, AppState>,
    id: String,
    detach: bool,
) -> CmdResult<TreeDto> {
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    haraka_core::manage::delete_credential(&mut vault.store, &id, detach).map_err(err)?;
    haraka_core::audit::append(vault.data_dir(), "credential_delete", Some(&id), None)
        .map_err(err)?;
    save_tree(vault)
}

/// Action explicite « afficher » : le secret sort une seule fois, journalisé.
#[tauri::command]
pub async fn credential_reveal(
    state: State<'_, AppState>,
    id: String,
    field: String,
) -> CmdResult<String> {
    let field = SecretField::parse(&field).map_err(err)?;
    let inner = state.inner.lock().await;
    let vault = inner.vault.as_ref().ok_or(LOCKED)?;
    let secret = vault.secret(&id, field).map_err(err)?;
    haraka_core::audit::append(
        vault.data_dir(),
        "credential_reveal",
        Some(&id),
        Some(field.as_str()),
    )
    .map_err(err)?;
    Ok(secret.expose_secret().to_string())
}

/// Action explicite « copier » : le secret va au presse-papier backend,
/// jamais au frontend ; effacement automatique après N secondes.
#[tauri::command]
pub async fn clipboard_copy_secret(
    state: State<'_, AppState>,
    id: String,
    field: String,
) -> CmdResult<u32> {
    let field = SecretField::parse(&field).map_err(err)?;
    let (secret, ttl) = {
        let inner = state.inner.lock().await;
        let vault = inner.vault.as_ref().ok_or(LOCKED)?;
        let secret = vault.secret(&id, field).map_err(err)?;
        haraka_core::audit::append(
            vault.data_dir(),
            "clipboard_copy_secret",
            Some(&id),
            Some(field.as_str()),
        )
        .map_err(err)?;
        (
            zeroize::Zeroizing::new(secret.expose_secret().to_string()),
            vault.store.settings.clipboard_clear_seconds,
        )
    };
    // Signal d'effacement anticipé (verrouillage du coffre) : voir `clear_clipboard_now`.
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
    *CLIP_CANCEL.lock().unwrap() = Some(cancel_tx);
    tauri::async_runtime::spawn_blocking(move || {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|e| format!("presse-papier indisponible : {e}"))?;
        // Indicateur « secret » (x-kde-passwordManagerHint) : Klipper et les gestionnaires d'historique
        // compatibles n'enregistrent pas la copie et ne la restaurent pas après l'effacement.
        {
            use arboard::SetExtLinux;
            clipboard
                .set()
                .exclude_from_history()
                .text(secret.to_string())
                .map_err(|e| format!("copie impossible : {e}"))?;
        }
        if ttl > 0 {
            // Linux : le contenu est hébergé par l'app — garder l'instance vivante pendant le TTL,
            // puis effacer si le contenu n'a pas changé. Un verrouillage du coffre écourte l'attente ;
            // une nouvelle copie remplace l'expéditeur : l'ancienne attente s'arrête sans rien effacer.
            match cancel_rx.recv_timeout(std::time::Duration::from_secs(u64::from(ttl))) {
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {}
                _ => {
                    // Contenu non textuel ou absent : un autre programme a pris la main, on n'y touche pas.
                    // Lecture impossible pour une autre raison : on efface par prudence.
                    let ours = match clipboard.get_text() {
                        Ok(t) => t == *secret,
                        Err(arboard::Error::ContentNotAvailable) => false,
                        Err(_) => true,
                    };
                    if ours && let Err(e) = clipboard.clear() {
                        tracing::warn!("effacement du presse-papier impossible : {e}");
                    }
                }
            }
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(ttl)
}

/// Dernière copie de secret en attente d'effacement automatique (signal d'effacement anticipé).
static CLIP_CANCEL: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>> =
    std::sync::Mutex::new(None);

/// Verrouillage du coffre : efface tout de suite le presse-papier s'il contient encore le dernier
/// secret copié (sans attendre la fin du délai). Ne touche à rien d'autre.
pub fn clear_clipboard_now() {
    if let Some(tx) = CLIP_CANCEL.lock().unwrap().take() {
        let _ = tx.send(());
    }
}

// ------------------------------------------------- M2 : rapide, macros, divers

#[tauri::command]
pub async fn quick_save(
    state: State<'_, AppState>,
    spec: String,
    name: Option<String>,
    folder_id: Option<String>,
) -> CmdResult<TreeDto> {
    let parsed = haraka_core::quick::parse(&spec).map_err(err)?;
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    let folder = match folder_id {
        Some(id) => id,
        None => {
            if let Some(root) = vault.store.folders.iter().find(|f| f.parent_id.is_none()) {
                root.id.clone()
            } else {
                let id = uuid::Uuid::new_v4().to_string();
                haraka_core::manage::upsert_folder(
                    &mut vault.store,
                    haraka_core::model::Folder {
                        id: id.clone(),
                        parent_id: None,
                        name: "Connexions rapides".to_string(),
                        color: None,
                        icon: None,
                        sort_order: 0,
                        tags: vec![],
                        defaults: Default::default(),
                    },
                )
                .map_err(err)?;
                id
            }
        }
    };
    let conn = haraka_core::quick::to_connection(
        &parsed,
        &name.unwrap_or_else(|| parsed.host.clone()),
        &folder,
    );
    haraka_core::manage::upsert_connection(&mut vault.store, conn).map_err(err)?;
    let new_id = vault
        .store
        .connections
        .last()
        .map(|c| c.id.clone())
        .ok_or("connexion introuvable après création")?;
    haraka_core::manage::touch_recent(&mut vault.store, &new_id).map_err(err)?;
    save_tree(vault)
}

#[tauri::command]
pub async fn ui_state_set(
    state: State<'_, AppState>,
    patch: serde_json::Value,
) -> CmdResult<TreeDto> {
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    if !vault.store.ui_state.is_object() {
        vault.store.ui_state = serde_json::json!({});
    }
    let (Some(base), Some(patch)) = (vault.store.ui_state.as_object_mut(), patch.as_object())
    else {
        return Err("patch ui_state invalide".into());
    };
    for (key, value) in patch {
        base.insert(key.clone(), value.clone());
    }
    save_tree(vault)
}

#[tauri::command]
pub async fn macro_upsert(
    state: State<'_, AppState>,
    id: Option<String>,
    name: String,
    content: Option<String>,
    confirm_before_send: bool,
) -> CmdResult<TreeDto> {
    if name.trim().is_empty() {
        return Err("nom de macro vide".into());
    }
    let is_new = id.is_none();
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    let macro_id = match id {
        Some(id) => {
            let m = vault
                .store
                .macros
                .iter_mut()
                .find(|m| m.id == id)
                .ok_or("macro introuvable")?;
            m.name = name;
            m.confirm_before_send = confirm_before_send;
            id
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            vault.store.macros.push(haraka_core::model::Macro {
                id: id.clone(),
                name,
                content: haraka_core::model::EncSecret::new(String::new(), String::new()),
                confirm_before_send,
            });
            id
        }
    };
    match content {
        Some(c) => vault
            .set_secret(&macro_id, SecretField::MacroContent, &SecretString::from(c))
            .map_err(err)?,
        None if is_new => return Err("contenu de macro requis à la création".into()),
        None => {}
    }
    save_tree(vault)
}

#[tauri::command]
pub async fn macro_delete(state: State<'_, AppState>, id: String) -> CmdResult<TreeDto> {
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    if !vault.store.macros.iter().any(|m| m.id == id) {
        return Err("macro introuvable".into());
    }
    vault.store.macros.retain(|m| m.id != id);
    for c in &mut vault.store.connections {
        if c.startup_snippet_id.as_deref() == Some(id.as_str()) {
            c.startup_snippet_id = None;
        }
    }
    save_tree(vault)
}

#[tauri::command]
pub async fn vault_recalibrate_kdf(state: State<'_, AppState>, master: String) -> CmdResult<()> {
    let mut inner = state.inner.lock().await;
    let vault = inner.vault.as_mut().ok_or(LOCKED)?;
    vault
        .recalibrate_kdf(&SecretString::from(master))
        .map_err(err)
}

/// Informations « À propos » : version, composants, licence MIT du projet
/// (texte embarqué à la compilation depuis `LICENSE`).
#[derive(Serialize)]
pub struct AboutDto {
    pub name: String,
    pub version: String,
    pub author: String,
    /// Hash git du build (`unknown` hors dépôt, suffixe `-dirty` si modifié).
    pub git_hash: String,
    pub description: String,
    pub tauri_version: String,
    pub os: String,
    pub arch: String,
    pub data_dir: String,
    pub license_name: String,
    pub license_text: String,
}

#[tauri::command]
pub async fn about_info(state: State<'_, AppState>) -> CmdResult<AboutDto> {
    Ok(AboutDto {
        name: "HarakaTS".into(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: env!("CARGO_PKG_AUTHORS").to_string(),
        git_hash: env!("HARAKA_GIT_HASH").to_string(),
        description: env!("CARGO_PKG_DESCRIPTION").to_string(),
        tauri_version: tauri::VERSION.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        data_dir: state.data_dir.root().display().to_string(),
        license_name: "MIT".into(),
        license_text: include_str!("../../LICENSE").to_string(),
    })
}

/// Avis de licence des composants tiers (Rust + npm), embarqués à la
/// compilation. Fichier généré par `cargo run -p xtask -- licenses`.
#[tauri::command]
pub async fn third_party_licenses() -> CmdResult<String> {
    Ok(include_str!("../../THIRD-PARTY-LICENSES.md").to_string())
}
