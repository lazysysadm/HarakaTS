// API IPC typée — aucun secret ne transite backend → frontend, sauf
// credential_reveal / clipboard_copy_secret (actions explicites, auditées).
import { invoke } from "@tauri-apps/api/core";
import type { FileEntry } from "./sftp";
import { translateError } from "./errors";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface VaultStatus {
  state: "uninitialized" | "locked" | "unlocked";
  keyring_available: boolean;
  keyring_cache_enabled: boolean;
  autolock_minutes: number;
  data_dir: string;
  vault_id: string | null;
  app_version: string;
}

export interface Counts {
  connections: number;
  credentials: number;
  folders: number;
  macros: number;
}

export interface Conflict {
  kind: "folder" | "connection" | "credential" | "macro";
  id: string;
  name: string;
}

export interface PreviewDto {
  preview_id: string;
  counts: Counts;
  conflicts: Conflict[];
  includes_secrets: boolean;
  source_vault_id: string;
}

export interface ExportResult {
  path: string;
  counts: Counts;
}

export interface BackupDto {
  index: number;
  modified_epoch: number | null;
  valid: boolean;
}

export interface ReconnectSettings {
  enabled: boolean;
  max_attempts: number;
  backoff_max_s: number;
}

export interface Settings {
  autolock_minutes: number;
  lock_on_suspend: boolean;
  lock_on_session_lock: boolean;
  disconnect_on_lock: boolean;
  keyring_cache: boolean;
  clipboard_default: string;
  osc52_max_bytes: number;
  paste_confirm_multiline: boolean;
  copy_on_select: boolean;
  clipboard_clear_seconds: number;
  locale: string;
  theme: string;
  terminal_font_size: number;
  terminal_scrollback: number;
  terminal_in_tabs: boolean;
  terminal_highlight: boolean;
  server_status: boolean;
  server_status_interval_s: number;
  external_editor: string | null;
  transfer_parallelism: number;
  sftp_follow_terminal: boolean;
  session_log_encrypt: boolean;
  session_log_retention_days: number;
  reconnect: ReconnectSettings;
}

export interface AuditEntry {
  ts: string;
  event: string;
  target: string | null;
  detail: string | null;
}

export interface StrategyOverride {
  key: string;
  strategy: string;
}

// ---------------------------------------------------------------- M2 : arbre

export interface JumpHop {
  host: string;
  port: number | null;
  user: string | null;
  credential_id: string | null;
}

export interface ConnectionDefaults {
  user: string | null;
  port: number | null;
  credential_id: string | null;
  jump_chain: JumpHop[];
  clipboard_policy: string | null;
  keepalive_s: number | null;
  encoding: string | null;
  auth_method_order: string[] | null;
}

export interface Folder {
  id: string;
  parent_id: string | null;
  name: string;
  color: string | null;
  icon: string | null;
  sort_order: number;
  tags: string[];
  defaults: ConnectionDefaults;
}

export interface TunnelSpec {
  id: string;
  kind: string;
  bind_addr: string;
  bind_port: number;
  dest_host: string | null;
  dest_port: number | null;
  auto_start: boolean;
}

export interface Connection {
  id: string;
  folder_id: string;
  name: string;
  protocol: string;
  host: string;
  port: number | null;
  user: string | null;
  credential_id: string | null;
  jump_chain: JumpHop[] | null;
  clipboard_policy: string | null;
  keepalive_s: number | null;
  encoding: string | null;
  color: string | null;
  icon: string | null;
  tags: string[];
  favorite: boolean;
  tunnels: TunnelSpec[];
  startup_snippet_id: string | null;
  session_logging: boolean;
  sftp: { follow_terminal_cwd: boolean; show_hidden: boolean };
  notes: string;
  created_at: string;
  updated_at: string;
  last_used_at: string | null;
}

export interface CredentialMeta {
  id: string;
  name: string;
  username: string;
  has_password: boolean;
  has_private_key: boolean;
  has_passphrase: boolean;
  needs_reentry: string[];
  ref_count: number;
  created_at: string;
  updated_at: string;
}

export interface MacroMeta {
  id: string;
  name: string;
  confirm_before_send: boolean;
}

export interface Recent {
  connection_id: string;
  last_used_at: string;
}

export interface TreeDto {
  folders: Folder[];
  connections: Connection[];
  credentials: CredentialMeta[];
  macros: MacroMeta[];
  recents: Recent[];
  ui_state: Record<string, unknown>;
}

export interface Provenance {
  kind: "system" | "folder" | "connection";
  id: string | null;
  name: string | null;
}

export interface Resolved<T> {
  value: T | null;
  source: Provenance;
}

export interface EffectiveConfig {
  host: string;
  user: Resolved<string>;
  port: Resolved<number>;
  credential_id: Resolved<string>;
  jump_chain: Resolved<JumpHop[]>;
  clipboard_policy: Resolved<string>;
  keepalive_s: Resolved<number>;
  encoding: Resolved<string>;
  auth_method_order: Resolved<string[]>;
}

// ---------------------------------------------------------------- M3 : sessions

export interface SessionInfo {
  id: string;
  connection_id: string;
  title: string;
  state: string;
}

export interface AgentStatus {
  available: boolean;
  keys: { algorithm: string; fingerprint: string; comment: string; certificate: boolean; hardware: boolean }[];
}

export interface LogFileDto {
  name: string;
  size: number;
  modified_epoch: number | null;
  encrypted: boolean;
}

export interface KnownHostDto {
  host_field: string;
  key_type: string;
  fingerprint: string;
}

export interface KeyInfo {
  algorithm: string;
  fingerprint: string;
  comment: string;
  encrypted: boolean;
}

export interface KeyImportDto {
  pending_id: string;
  info: KeyInfo;
  file_name: string;
  public_key: string | null;
  permissions_warning: string | null;
}

export const api = {
  status: () => invoke<VaultStatus>("vault_status"),
  create: (master: string, confirm: string, keyringCache: boolean) =>
    invoke<VaultStatus>("vault_create", { master, confirm, keyringCache }),
  unlock: (master: string | null, useKeyring: boolean) =>
    invoke<VaultStatus>("vault_unlock", { master, useKeyring }),
  lock: () => invoke<VaultStatus>("vault_lock"),
  changeMaster: (old: string, next: string, confirm: string) =>
    invoke<void>("vault_change_master", { old, new: next, confirm }),
  keyringSetEnabled: (enabled: boolean) =>
    invoke<VaultStatus>("vault_keyring_set_enabled", { enabled }),
  activity: () => invoke<void>("vault_activity"),
  listBackups: () => invoke<BackupDto[]>("vault_list_backups"),
  vaultIntegrity: () => invoke<boolean>("vault_integrity"),
  restoreBackup: (index: number) => invoke<VaultStatus>("vault_restore_backup", { index }),
  settingsGet: () => invoke<Settings>("settings_get"),
  settingsSet: (patch: Partial<Settings>) => invoke<Settings>("settings_set", { patch }),
  exportBundle: (scope: string, folderId: string | null, passphrase: string, confirm: string) =>
    invoke<ExportResult>("export_bundle", { scope, folderId, passphrase, confirm }),
  importPreview: (passphrase: string) =>
    invoke<PreviewDto>("import_pick_and_preview", { passphrase }),
  importApply: (previewId: string, defaultStrategy: string, overrides: StrategyOverride[]) =>
    invoke<Counts>("import_apply", { previewId, defaultStrategy, overrides }),
  audit: (limit: number) => invoke<AuditEntry[]>("audit_log_list", { limit }),
  onState: (cb: () => void): Promise<UnlistenFn> => listen("vault://state", () => cb()),

  // M2
  treeGet: () => invoke<TreeDto>("tree_get"),
  folderUpsert: (folder: Folder) => invoke<TreeDto>("folder_upsert", { folder }),
  folderDelete: (id: string, cascade: boolean) => invoke<TreeDto>("folder_delete", { id, cascade }),
  folderMove: (id: string, newParent: string | null, sortOrder: number | null) =>
    invoke<TreeDto>("folder_move", { id, newParent, sortOrder }),
  connectionUpsert: (connection: Connection) =>
    invoke<TreeDto>("connection_upsert", { connection }),
  connectionDelete: (id: string) => invoke<TreeDto>("connection_delete", { id }),
  connectionDuplicate: (id: string) => invoke<TreeDto>("connection_duplicate", { id }),
  connectionEffective: (id: string) => invoke<EffectiveConfig>("connection_effective", { id }),
  credentialUpsert: (
    id: string | null,
    name: string,
    username: string,
    password?: string | null,
    privateKey?: string | null,
    passphrase?: string | null,
    pendingKeyId?: string | null,
  ) =>
    invoke<TreeDto>("credential_upsert", {
      id,
      name,
      username,
      password,
      privateKey,
      passphrase,
      pendingKeyId,
    }),
  credentialImportKeyFile: () => invoke<KeyImportDto>("credential_import_key_file"),
  credentialDelete: (id: string, detach: boolean) =>
    invoke<TreeDto>("credential_delete", { id, detach }),
  credentialReveal: (id: string, field: string) =>
    invoke<string>("credential_reveal", { id, field }),
  clipboardCopySecret: (id: string, field: string) =>
    invoke<number>("clipboard_copy_secret", { id, field }),
  quickSave: (spec: string, name: string | null, folderId: string | null) =>
    invoke<TreeDto>("quick_save", { spec, name, folderId }),
  uiStateSet: (patch: Record<string, unknown>) => invoke<TreeDto>("ui_state_set", { patch }),
  macroUpsert: (
    id: string | null,
    name: string,
    content: string | null,
    confirmBeforeSend: boolean,
  ) => invoke<TreeDto>("macro_upsert", { id, name, content, confirmBeforeSend }),
  macroDelete: (id: string) => invoke<TreeDto>("macro_delete", { id }),
  recalibrateKdf: (master: string) => invoke<void>("vault_recalibrate_kdf", { master }),

  // M3

  sessionOpenVte: (connectionId: string) =>
    invoke<string>("session_open_vte", { connectionId }),
  sessionFocusVte: (id: string) => invoke<void>("session_focus_vte", { id }),

  sessionClose: (id: string) => invoke<void>("session_close", { id }),
  sessionReconnect: (id: string) => invoke<void>("session_reconnect", { id }),
  sessionList: () => invoke<SessionInfo[]>("session_list"),
  /** Mode onglets : colonne du tableau de bord large (vue ouverte) ou étroite. */
  layoutWide: (wide: boolean) => invoke<void>("layout_wide", { wide }),
  /** Événements de cycle de vie / focus des fenêtres terminal natives. */
  onSessionEvent: (
    kind: "opened" | "closed" | "active" | "state",
    cb: (session: string) => void,
  ): Promise<UnlistenFn> =>
    listen<{ session: string }>(`session://${kind}`, (e) => cb(e.payload.session)),
  authPromptRespond: (promptId: string, answers: string[]) =>
    invoke<void>("auth_prompt_respond", { promptId, answers }),
  hostkeyPromptRespond: (promptId: string, accept: boolean, remember: boolean) =>
    invoke<void>("hostkey_prompt_respond", { promptId, accept, remember }),
  knownHostsList: () => invoke<KnownHostDto[]>("known_hosts_list"),
  knownHostsImportSystem: () => invoke<number>("known_hosts_import_system"),
  sshAgentStatus: () => invoke<AgentStatus>("ssh_agent_status"),
  sessionLogsList: () => invoke<LogFileDto[]>("session_logs_list"),
  sessionLogRead: (name: string) => invoke<string>("session_log_read", { name }),
  sessionLogDelete: (name: string) => invoke<void>("session_log_delete", { name }),
  sessionLogsPurge: (days: number | null) => invoke<number>("session_logs_purge", { days }),
  knownHostsForget: (hostField: string, fingerprint: string) =>
    invoke<number>("known_hosts_forget", { hostField, fingerprint }),
  knownHostsClear: () => invoke<number>("known_hosts_clear"),
  clipboardCopyText: (text: string) => invoke<void>("clipboard_copy_text", { text }),

};

export interface AboutInfo {
  name: string;
  version: string;
  author: string;
  git_hash: string;
  description: string;
  tauri_version: string;
  os: string;
  arch: string;
  data_dir: string;
  license_name: string;
  license_text: string;
}

export const aboutApi = {
  info: () => invoke<AboutInfo>("about_info"),
  thirdParty: () => invoke<string>("third_party_licenses"),
};

// M5 — SFTP et transferts
export type TransferDirection = "upload" | "download";
export type TransferState = "queued" | "running" | "paused" | "done" | "error" | "cancelled";

export interface TransferSpec {
  session: string;
  direction: TransferDirection;
  source: string;
  dest_dir: string;
  conflict?: "skip" | "overwrite";
  symlinks?: "skip" | "follow";
  preserve?: boolean;
}

export interface TransferInfo {
  id: number;
  session: string;
  direction: TransferDirection;
  source: string;
  dest_dir: string;
  name: string;
  state: TransferState;
  error: string | null;
  done: number;
  total: number;
  files_done: number;
  files_total: number;
  bps: number;
  eta_s: number | null;
  current: string | null;
}

export interface Listing {
  path: string;
  entries: FileEntry[];
}

export const sftpApi = {
  home: (session: string) => invoke<string>("sftp_home", { session }),
  list: (session: string, path: string) => invoke<Listing>("sftp_list", { session, path }),
  stat: (session: string, path: string) => invoke<FileEntry>("sftp_stat", { session, path }),
  mkdir: (session: string, path: string) => invoke<void>("sftp_mkdir", { session, path }),
  rename: (session: string, from: string, to: string) =>
    invoke<void>("sftp_rename", { session, from, to }),
  remove: (session: string, path: string, recursive: boolean) =>
    invoke<void>("sftp_remove", { session, path, recursive }),
  chmod: (session: string, path: string, mode: number, recursive: boolean) =>
    invoke<void>("sftp_chmod", { session, path, mode, recursive }),
  symlink: (session: string, linkPath: string, target: string) =>
    invoke<void>("sftp_symlink", { session, linkPath, target }),
  localHome: () => invoke<string>("local_home"),
  localList: (path: string) => invoke<Listing>("local_list", { path }),
  localMkdir: (path: string) => invoke<void>("local_mkdir", { path }),
  pickFiles: () => invoke<string[]>("sftp_pick_files"),
  pickFolder: () => invoke<string | null>("sftp_pick_folder"),
  shellSnippet: () => invoke<string>("sftp_shell_snippet"),
  enableFollow: (session: string) => invoke<void>("sftp_enable_follow", { session }),
  enqueue: (specs: TransferSpec[]) => invoke<number[]>("transfer_enqueue", { specs }),
  pause: (id: number) => invoke<void>("transfer_pause", { id }),
  resume: (id: number) => invoke<void>("transfer_resume", { id }),
  cancel: (id: number) => invoke<void>("transfer_cancel", { id }),
  transfers: () => invoke<TransferInfo[]>("transfer_list"),
  clearTransfers: () => invoke<void>("transfer_clear"),
  onTransfer: (cb: (info: TransferInfo) => void): Promise<UnlistenFn> =>
    listen<TransferInfo>("transfer://event", (e) => cb(e.payload)),
  onOpen: (cb: (session: string) => void): Promise<UnlistenFn> =>
    listen<{ session: string }>("sftp://open", (e) => cb(e.payload.session)),
  onCwd: (cb: (session: string, path: string) => void): Promise<UnlistenFn> =>
    listen<{ session: string; path: string }>("sftp://cwd", (e) =>
      cb(e.payload.session, e.payload.path),
    ),
};

// ---------------------------------------------------------------- M6 : édition distante

export type LineEnding = "lf" | "crlf" | "mixed" | "none";

export interface EditDocInfo {
  id: string;
  session: string;
  remote_path: string;
  name: string;
  size: number;
  mtime: number | null;
  mode: number | null;
  line_ending: LineEnding;
  utf8: boolean;
  external: boolean;
}

/** `content` nul : fichier non UTF-8 ou trop gros pour l'édition intégrée. */
export interface EditOpenDto {
  info: EditDocInfo;
  content: string | null;
}

export interface RemoteState {
  exists: boolean;
  size: number;
  mtime: number | null;
}

export type SaveOutcome =
  | { status: "saved"; info: EditDocInfo }
  | { status: "conflict"; remote: RemoteState };

export type EditEvent =
  | { type: "saved"; id: string; info: EditDocInfo }
  | { type: "conflict"; id: string; remote: RemoteState }
  | { type: "error"; id: string; message: string };

export const editApi = {
  open: (session: string, path: string) => invoke<EditOpenDto>("edit_open", { session, path }),
  save: (id: string, content: string, overwrite: boolean) =>
    invoke<SaveOutcome>("edit_save", { id, content, overwrite }),
  saveLocal: (id: string, overwrite: boolean) =>
    invoke<SaveOutcome>("edit_save_local", { id, overwrite }),
  remoteContent: (id: string) => invoke<string | null>("edit_remote_content", { id }),
  localContent: (id: string) => invoke<string | null>("edit_local_content", { id }),
  reload: (id: string) => invoke<EditOpenDto>("edit_reload", { id }),
  openExternal: (id: string) => invoke<void>("edit_open_external", { id }),
  close: (id: string) => invoke<void>("edit_close", { id }),
  list: () => invoke<EditDocInfo[]>("edit_list"),
  onEvent: (cb: (ev: EditEvent) => void): Promise<UnlistenFn> =>
    listen<EditEvent>("edit://event", (e) => cb(e.payload)),
};

export function message(e: unknown): string {
  return translateError(
    typeof e === "string" ? e : e instanceof Error ? e.message : String(e),
  );
}

// ---- M7 : imports depuis d'autres outils (ssh_config, Remmina, MobaXterm)

export type ExternalSource = "ssh_config" | "remmina" | "mobaxterm";

/** Avertissement structuré : traduit côté UI via `import.warn.<code>`. */
export interface ImportWarning {
  code: string;
  params: Record<string, string>;
}

export interface ExternalPreview {
  preview_id: string;
  source: ExternalSource;
  root_folder: string;
  folders: string[];
  connections: {
    name: string;
    host: string;
    port: number | null;
    user: string | null;
    folder: string;
    jumps: number;
    tunnels: number;
  }[];
  credentials: number;
  keys: { path: string; status: "ok" | "encrypted" | "unreadable" }[];
  secrets_to_reenter: number;
  skipped: number;
  warnings: ImportWarning[];
}

export interface ExternalReport {
  source: ExternalSource;
  root_folder: string;
  imported: number;
  folders: number;
  credentials: number;
  skipped: number;
  secrets_to_reenter: number;
  warnings: ImportWarning[];
}

export const importExternalApi = {
  /** Ouvre le dialogue natif, analyse (sans rien écrire) ; secrets/clés restent côté backend. */
  preview: (source: ExternalSource, directory = false) =>
    invoke<ExternalPreview>("import_external_preview", { source, directory }),
  apply: (previewId: string) => invoke<ExternalReport>("import_external_apply", { previewId }),
};

// M7 : multi-exec et macros

export interface MultiExecState {
  enabled: boolean;
  sessions: string[];
}

export interface MacroPreview {
  /** Aperçu exact, caractères de contrôle rendus visibles (␛ ␍ ␊). */
  preview: string;
  lines: number;
  bytes: number;
}

export const multiExecApi = {
  state: () => invoke<MultiExecState>("multi_exec_state"),
  set: (enabled: boolean, sessions: string[]) =>
    invoke<MultiExecState>("multi_exec_set", { enabled, sessions }),
  /** Envoie une ligne (+ Entrée) à toutes les sessions membres. */
  send: (text: string) => invoke<number>("multi_exec_send", { text }),
  onState: (cb: (s: MultiExecState) => void): Promise<UnlistenFn> =>
    listen<MultiExecState>("multiexec://state", (e) => cb(e.payload)),
};

export const statusApi = {
  snapshot: () => invoke<Record<string, boolean>>("status_snapshot"),
  /** Relance un cycle de test tout de suite. */
  refresh: () => invoke<void>("status_refresh"),
  onUpdate: (cb: (map: Record<string, boolean>) => void): Promise<UnlistenFn> =>
    listen<Record<string, boolean>>("status://update", (e) => cb(e.payload)),
};

export const macroApi = {
  /** Action explicite « afficher » (auditée) : seul chemin du contenu vers l'UI. */
  reveal: (id: string) => invoke<string>("macro_reveal", { id }),
  preview: (id: string) => invoke<MacroPreview>("macro_preview", { id }),
  /** `confirmed: true` uniquement après l'aperçu exact ; le backend l'exige si la macro le demande. */
  send: (macroId: string, session: string, confirmed: boolean) =>
    invoke<void>("macro_send", { macroId, session, confirmed }),
};

// M7 : tunnels SSH

export type TunnelKind = "local" | "remote" | "dynamic";
export type TunnelState = "starting" | "listening" | "error";

/** Instantané d'un tunnel vivant (backend `TunnelInfo`). */
export interface TunnelInfo {
  id: string;
  session: string;
  kind: TunnelKind;
  bind_addr: string;
  bind_port: number;
  dest_host: string;
  dest_port: number;
  state: TunnelState;
  error: string | null;
  active: number;
  bytes_out: number;
  bytes_in: number;
}

export type TunnelEvent =
  | { type: "changed"; info: TunnelInfo }
  | { type: "removed"; session: string; id: string };

/** Tunnel ad hoc (non enregistré dans la connexion). */
export interface AdHocTunnel {
  kind: TunnelKind;
  bind_addr: string;
  bind_port: number;
  dest_host: string | null;
  dest_port: number | null;
}

export const tunnelApi = {
  list: () => invoke<TunnelInfo[]>("tunnel_list"),
  /** Démarre une spec enregistrée de la connexion de la session. */
  start: (session: string, tunnelId: string) =>
    invoke<TunnelInfo>("tunnel_start", { session, tunnelId }),
  startAdHoc: (session: string, spec: AdHocTunnel) =>
    invoke<TunnelInfo>("tunnel_start", { session, spec }),
  stop: (session: string, tunnelId: string) => invoke<void>("tunnel_stop", { session, tunnelId }),
  onState: (cb: (ev: TunnelEvent) => void): Promise<UnlistenFn> =>
    listen<TunnelEvent>("tunnel://state", (e) => cb(e.payload)),
};
