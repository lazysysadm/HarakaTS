#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod edit;
mod i18n;
mod import_external;
mod lockwatch;
mod macros;
mod multiexec;
mod post_connect;
mod sessions;
mod sftp;
mod state;
mod status;
mod tunnels;
#[cfg(target_os = "linux")]
mod vte;
mod window_state;

use std::path::PathBuf;

use state::AppState;
use tauri::Manager;

fn main() {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!(
            "HarakaTS {} ({})",
            env!("CARGO_PKG_VERSION"),
            env!("HARAKA_GIT_HASH")
        );
        return;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    tauri::Builder::default()
        .setup(|app| {
            let dd = haraka_core::DataDir::open(resolve_data_dir())?;
            window_state::restore_and_track(app, &dd);
            for warning in haraka_core::store::check_permissions(&dd) {
                tracing::warn!("permissions : {warning}");
            }
            let _ = haraka_core::audit::append(&dd, "app_start", None, None);
            let (manager, events_rx) = sessions::new_manager();
            sessions::spawn_event_forwarder(events_rx, app.handle().clone());
            let (transfers, transfers_rx) = sftp::new_transfers(manager.clone());
            sftp::spawn_transfer_forwarder(transfers_rx, app.handle().clone());
            let (edits, edits_rx) = edit::new_edits(manager.clone(), dd.edit_tmp_dir());
            edit::spawn_edit_forwarder(edits_rx, app.handle().clone());
            app.manage(AppState::new(dd, manager, transfers, edits));
            let _ = app.state::<AppState>().app_handle.set(app.handle().clone());
            tunnels::spawn_forwarder(app.handle().clone());
            lockwatch::spawn(app.handle().clone());
            status::spawn(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::vault_status,
            commands::about_info,
            commands::third_party_licenses,
            commands::vault_create,
            commands::vault_unlock,
            commands::vault_lock,
            commands::vault_change_master,
            commands::vault_keyring_set_enabled,
            commands::vault_activity,
            commands::vault_list_backups,
            commands::vault_integrity,
            commands::vault_restore_backup,
            commands::settings_get,
            commands::settings_set,
            commands::export_bundle,
            commands::import_pick_and_preview,
            commands::import_apply,
            import_external::import_external_preview,
            import_external::import_external_apply,
            commands::audit_log_list,
            commands::tree_get,
            commands::folder_upsert,
            commands::folder_delete,
            commands::folder_move,
            commands::connection_upsert,
            commands::connection_delete,
            commands::connection_duplicate,
            commands::connection_effective,
            commands::credential_upsert,
            commands::credential_import_key_file,
            commands::credential_delete,
            commands::credential_reveal,
            commands::clipboard_copy_secret,
            commands::quick_save,
            commands::ui_state_set,
            commands::macro_upsert,
            commands::macro_delete,
            macros::macro_reveal,
            macros::macro_preview,
            macros::macro_send,
            multiexec::multi_exec_set,
            multiexec::multi_exec_state,
            multiexec::multi_exec_send,
            commands::vault_recalibrate_kdf,
            sessions::session_open_vte,
            sessions::session_focus_vte,
            sessions::session_close,
            sessions::session_reconnect,
            sessions::session_list,
            sessions::layout_wide,
            status::status_snapshot,
            status::status_refresh,
            sessions::auth_prompt_respond,
            sessions::hostkey_prompt_respond,
            sessions::known_hosts_list,
            sessions::known_hosts_import_system,
            sessions::known_hosts_forget,
            sessions::known_hosts_clear,
            sessions::ssh_agent_status,
            sessions::session_logs_list,
            sessions::session_log_read,
            sessions::session_log_delete,
            sessions::session_logs_purge,
            sessions::clipboard_copy_text,
            sftp::sftp_home,
            sftp::sftp_list,
            sftp::sftp_stat,
            sftp::sftp_mkdir,
            sftp::sftp_rename,
            sftp::sftp_remove,
            sftp::sftp_chmod,
            sftp::sftp_symlink,
            sftp::local_home,
            sftp::local_list,
            sftp::local_mkdir,
            sftp::sftp_pick_files,
            sftp::sftp_pick_folder,
            sftp::sftp_shell_snippet,
            sftp::sftp_enable_follow,
            sftp::transfer_enqueue,
            sftp::transfer_pause,
            sftp::transfer_resume,
            sftp::transfer_cancel,
            sftp::transfer_list,
            sftp::transfer_clear,
            edit::edit_open,
            edit::edit_save,
            edit::edit_save_local,
            edit::edit_remote_content,
            edit::edit_local_content,
            edit::edit_reload,
            edit::edit_open_external,
            edit::edit_close,
            edit::edit_list,
            tunnels::tunnel_start,
            tunnels::tunnel_stop,
            tunnels::tunnel_list,
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de HarakaTS");
}

/// `--data-dir <chemin>` > mode portable (marqueur à côté du binaire) > XDG.
fn resolve_data_dir() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--data-dir")
        && let Some(path) = args.get(pos + 1)
    {
        return PathBuf::from(path);
    }
    if let Some(path) = haraka_core::DataDir::portable_path() {
        return path;
    }
    haraka_core::DataDir::default_path()
}
