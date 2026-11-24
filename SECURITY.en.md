# SECURITY — HarakaTS v1

> Phase 0 — threat model and security policy. To be validated before any code is written.
> Complements: [EXPORT_FORMAT.en.md](EXPORT_FORMAT.en.md) (binary format), [ARCHITECTURE.en.md](ARCHITECTURE.en.md) §5.

## 1. Threat model

| # | Scenario | Protection | Expected outcome |
|---|---|---|---|
| T1 | **Vault file theft** (disk, backup, cloud, USB stick) | Argon2id (parameters calibrated to 0.5–1 s) + XChaCha20-Poly1305; random 256-bit DEK; no plaintext secret on disk | Offline attack = Argon2id cost per guess; without the master password, nothing is decryptable. The OS keyring adds **no** decryption path off this machine |
| T2 | **Compromised workstation, unlocked vault** (user-level malware) | Acknowledged limits: the DEK and secrets live in process memory; zeroize/secrecy shrink the window but an attacker running as the same user can read memory | **Not covered** (see §8). Mitigations: auto-lock on inactivity/sleep/session lock, keyring cache disabled by default, reveal/copy audited |
| T3 | **Intercepted export** (network, mail, sharing) | Bundle re-encrypted with an **export passphrase distinct** from the master password, Argon2id + XChaCha20-Poly1305; "no secrets" option | Without the export passphrase the bundle is useless; the master password is never reused or transmitted |
| T4 | **Export tampering** (bit flip, block replacement, secret permutation) | AEAD over the whole envelope (AAD = canonical header) + per-secret AEAD (AAD = vault_id ∥ entry_id ∥ field) | Any alteration → tag failure → **complete rejection**, no partial decryption. Swapping two secrets across entries/fields also fails (AAD) |
| T5 | SSH MITM (host impersonation) | TOFU with displayed SHA256 fingerprint, internal known_hosts, **hard block** if the key changes, no automatic acceptance | The user decides explicitly; a key change = refusal by default + audit |
| T6 | Malicious paste (pastejacking: commands hidden in a paste) | Bracketed paste, multiline confirmation, control-character/escape detection, exact content preview | The user sees what will be sent before it goes out |
| T7 | Leak via UI / IPC / logs | Secrets never sent to the frontend except explicit reveal/copy; sanitized DTOs; `secrecy` (redacted Debug); backend logs without auth material; strict CSP, no remote URL | No secret in events, panics, dumps or logs |
| T8 | Persistent clipboard (Klipper/GNOME history) | Automatic clearing after N s (default 30) | **Acknowledged limit**: a history manager may have captured the value (§8) |
| T9 | OS keyring abuse | **Opt-in** cache, disabled by default; the DEK wrapped by the master password always remains in the header | Without Secret Service (KWallet absent) the app works; the keyring is never required to decrypt |

## 2. Key hierarchy and encryption

```
mot de passe maître ──Argon2id(sel, m, t, p)──▶ KEK (256 bits)
                                                 │ wrap (XChaCha20-Poly1305, AAD="HRKTS/DEK"∥vault_id)
                                                 ▼
DEK (256 bits, OsRng) ───────────────────────────┤
   │                                             │
   ├─ couche 1 : payload entier du store         │  AAD = magic ∥ version ∥ header_len ∥ header
   │            (JSON : connexions, dossiers,    │
   │             credentials, métadonnées)       │
   │                                             │
   └─ couche 2 : chaque champ secret             │  AAD = vault_id ∥ entry_id ∥ field_name
                (password, private_key,          │  (canonique, préfixes de longueur u32be)
                 passphrase, contenu de macro)   │
                nonce 24 o aléatoire par chiffrement
```

- **Deliberate double layer**: layer 1 protects metadata confidentiality (hosts, users,
  topology — sensitive for a sysadmin); layer 2 binds each secret to its entry and field
  (anti-permutation, a prompt requirement) and enables the "no secrets" export as well as
  field-by-field re-encryption on import.
- **Randomness**: `rand 0.10` OsRng (rand_core 0.10, aligned with aead 0.6 — no mixing of
  rand_core lineages). XChaCha20 nonces: 24 random bytes, unique per message (safe up to
  ~2^80 messages per key; DEK rotation possible post-v1).
- **KDF**: Argon2id v0x13, raw API `argon2 0.6::hash_password_into`. Calibration at
  creation: m = 131072 KiB (128 MiB), p = 4, t adjusted (2→8) to reach 0.5–1 s; on a slow
  machine, m is reduced in steps (floor 64 MiB); parameters stored in the header,
  re-calibratable. 16-byte random salt, **new salt on every master password change**.
- **Master password change**: verify the old one (unwrap DEK), new salt + new KEK,
  **re-wrap of the DEK only**; payload and secrets stay encrypted as-is → O(1) operation,
  no vault rewrite.
- **In memory**: DEK in `SecretBox<[u8; 32]>`, passwords/passphrases in
  `SecretString`/`SecretBox`, decryption buffers `zeroize`d after use (including temporary
  copies). `Debug`/`Display` redacted via `secrecy`. No secret in error messages: crypto
  errors are typed (`WrongPassword`, `Tampered`, `UnsupportedVersion`) with no sensitive
  content.

## 3. OS keyring: a convenience cache, never a dependency

- Backend: `keyring 3.6.2`, features `sync-secret-service` + `crypto-rust` + `vendored`
  → D-Bus Secret Service (KWallet on KDE, GNOME Keyring, KeePassXC…).
- Content: the raw DEK (`set_secret`, binary), entry `service="harakats"`,
  `user=<vault_id>`. **Acknowledged consequence**: on this machine, with an open session,
  a process able to query the Secret Service can unlock the vault without the master
  password. That is the price of convenience — disabled by default, explicitly enableable,
  disableable at any time (the entry is deleted).
- **Never required**: the header always contains the DEK wrapped by the master KEK.
  Missing service, locked service, D-Bus error, unconfigured KWallet → **silent,
  non-blocking** fallback to the master password (user requirement). No blocking error
  message at startup if the keyring is absent.
- Changing the master password does not change the DEK: the keyring cache stays valid
  (documented in the UI). Deleting the cache = `delete_credential`.
- The keyring stores **nothing else** (no connection passwords, no export passphrase):
  everything else lives in the vault.

## 4. Secret hygiene and IPC

- Forbidden direction: backend → frontend, except `credential_reveal(id, field)` and
  `clipboard_copy_secret(id, field, ttl)` — explicit user actions, audited, value returned
  once, never cached on the UI side.
- Allowed direction: frontend → backend when the user types a secret (credential forms,
  master password, export passphrase, keyboard-interactive answers). These values travel
  over Tauri IPC (local channel, no network) and are immediately wrapped in `secrecy` on
  the backend side.
- The backend authenticates **itself**: russh receives material from the in-memory
  `SecretResolver`, never from the UI.
- Clipboard: copying a secret → automatic clearing after N seconds (default 30,
  adjustable, 0 = no clearing with a warning). The Linux primary selection is cleared too.
- **OSC 52 (M4)**: intercepted in the backend before the terminal (native VTE or xterm) —
  no OSC 52 sequence ever reaches the renderer. Remote→local write: `r2l`/`bidirectional`
  policy + `osc52_max_bytes` limit (default 100 KB), audited (size, never content).
  **Query (the remote reads the local clipboard): `l2r`/`bidirectional` policy, always
  audited** — an exfiltration vector; on untrusted hosts use `r2l` or `disabled`. Refused
  operations are logged.
- **Paste (M4)**: multiline or control characters/escapes (beyond edge bracketed-paste
  markers) → confirmation with exact preview (visible controls), anti-pastejacking; an
  internal bracketed-paste marker is treated as suspicious. Cancel sends nothing.
  Setting `paste_confirm_multiline` (on by default).
- Logs: `tracing` with redacted secret fields; no secret value, DEK or passphrase in logs,
  errors or panics. Panics are intercepted (hook) to log a generic message. Dumps/core
  dumps cannot be disabled by the app — acknowledged limit (§8).
- **Terminal session logging** (opt-in per connection, off by default): the raw stream may
  contain secrets echoed by the remote shell (e.g. a password typed in clear text).
  Explicit warning on enable; 0600 files; configurable purge. No backend authentication
  material is ever written there.

## 5. Tauri / webview hardening

- Strict CSP (ARCHITECTURE.en.md §13): `script-src 'self'`, no remote URL,
  `object-src 'none'`, `frame-ancestors 'none'`. Inline styles allowed only
  (xterm/CodeMirror inject dynamic styles).
- Minimal capabilities: commands declared one by one, `core:event`, window creation for
  detached tabs. **No fs/shell/http/dialog/opener plugin**. File dialogs: `rfd` on the
  backend side. External editor: `std::process::Command` on the backend side,
  user-configurable command (no shell injection: direct argv execution, never `sh -c` on
  an interpolated string).
- `devtools` in dev builds only. `dangerousDisableAssetCspModification`: no.
- Validation of every IPC input (serde types + bounds: sizes, paths, UUIDs); SFTP/local
  paths are normalized on the backend side; no directory traversal allowed for remote
  editing outside the private temp.
- OS → app drag & drop: local file paths only, never URLs loaded into the webview.

## 6. Files and permissions

| Path | Mode | Content |
|---|---|---|
| `data/` (directory) | 0700 | everything else |
| `vault.hrk` (+ `.tmp`, backups) | 0600 | header + encrypted payload |
| `known_hosts` | 0600 | host public keys (not secret, integrity matters) |
| `audit.log` | 0600 | sensitive events, no secrets |
| `logs/<session>.log` | 0600 | opt-in logging |
| `tmp/<uuid>/` | 0700 (files 0600) | temporary remote-editing copies, purged on close and at startup |

- Atomic writes (tmp + fsync + rename); umask checked at startup; if the data directory
  exists with overly broad permissions → warning + proposed fix (never silent).
- Portable mode: same permissions on removable media (warning if the FS does not support
  them, e.g. FAT).

## 7. SSH security

- Host keys: TOFU + SHA256 fingerprint in OpenSSH format; **key change = hard block**:
  the session is refused with no acceptance button at all (no answerable prompt is
  created); replacement goes through **explicit forgetting** of the entry (Settings →
  Host keys (internal known_hosts) → Forget, audit `known_hosts_forget`), then a new TOFU
  first connection; system known_hosts import (hashed entries and `@revoked` markers
  supported: a revoked key is refused without a prompt). **Fail-closed**: an unreadable
  known_hosts (I/O error, invalid bytes) refuses the connection instead of falling back
  to a TOFU first connection; a corrupted line for the host counts as a changed key. No
  implicit `StrictHostKeyChecking=no`, no "always accept" option.
- Algorithms: `russh::Preferred` by default (modern kex, ed25519/ecdsa/rsa-sha2); no weak
  algorithms manually reintroduced.
- Private keys: imported **into the vault** (encrypted), never left in clear text in the
  temp; encrypted keys are accepted (passphrase in the vault).
- Local SSH agent: read via `SSH_AUTH_SOCK` for authentication (tried first; the identity
  list, public keys only, is visible in Settings); **FIDO2 hardware keys (`sk-…`)** work
  through the agent: the private key never leaves the token and nothing enters the vault;
  **agent forwarding disabled and not offered in v1** (a compromised remote server could
  use the agent to pivot).
- Keyboard-interactive: prompts are displayed as-is (server banner included, rendered as
  plain text — never HTML/automatically clickable URI); answers (OTP) are never logged.
- Tunnels: default bind `127.0.0.1` (not `0.0.0.0`); exposing a remote tunnel depends on
  the server's `GatewayPorts`, shown in the UI; dynamic SOCKS5 without authentication →
  loopback-only bind by default, warning on non-loopback bind.
- Automatic reconnection: only while the vault is unlocked; on lock, sessions are closed
  (default) and material is zeroized.

### 7.1 SFTP and transfers (M5)

- **Same connection, same authentication**: the SFTP channel is a second channel of the
  terminal's SSH session; no secret is re-entered or re-exposed to the frontend.
- **Stream isolation**: only the shell channel feeds the terminal display (filtering by
  channel id in the handler) — an SFTP response can neither appear nor be interpreted as
  escape sequences by the terminal. Dedicated regression test.
- **Paths**: always passed as-is to the SFTP protocol (never through a shell); no path or
  file content in the audit log (`sftp_transfer`: direction only, `sftp_remove`: recursive
  option only).
- **Atomic writes**: `<file>.part` then rename; resume only if the trailing 64 KiB match
  (modified source / corrupted `.part` → restart from zero). Cancel deletes the `.part`
  of the file in progress.
- **Symbolic links**: ignored by default in recursive transfers; "follow" is opt-in,
  bounded depth (64) against loops; recursive `chmod` and recursive `rm` never follow a
  link.
- **Local panel**: `local_list` exposes a listing of the user's filesystem to the webview
  (read-only, no content reading); the CSP and Tauri capabilities remain unchanged (no
  `fs` plugin).
- **OSC 7**: the directory advertised by the server is untrusted data: it only navigates
  the panel (never executed).
- **Follow-hook injection (opt-in)**: the SFTP panel's "Follow terminal" checkbox
  (checking = explicit action) or the `sftp_follow_terminal` setting (off by default) TYPE
  a fixed line, compiled into the application (never built from remote or user data),
  into the session's interactive shell — equivalent to typing it yourself. The echo is
  hidden locally (SGR "conceal" then erase): this is display comfort, NOT a protection
  (the server sees the line, which contains no secret). No remote file modified; action
  audited (`sftp_follow_enable`). Acknowledged risk: if a fullscreen program is in the
  foreground, the line is sent to it (hence the "at the prompt" confirmation); with the
  automatic option, it is sent ~600 ms after connection. Injection-free alternative: copy
  the snippet.
- **Limit**: transfer integrity is verified by size (SFTP v3 without a standard hash);
  network confidentiality/integrity is that of the SSH transport.

### 7.2 Remote editing (M6)

- **Plaintext content while editing**: the remote file is downloaded into
  `$XDG_RUNTIME_DIR/harakats-edit-<id>/<uuid>/` (**tmpfs**, never on disk; fallback to
  `<data>/tmp/` without a runtime dir), directory **0700**, file **0600**. On document
  close, vault lock and startup (crash), files are **overwritten with zeros then deleted**
  (best effort, no guarantee on SSD/journaling). An external editor may create copies
  (backups, swap) outside our control: avoid for sensitive files.
- **No silent overwrite**: the remote state (mtime, size, SHA-256) is compared before
  every send; any concurrent change is an explicit conflict (overwrite / reload /
  compare).
- **External editor launched without a shell**: the settings command is split into
  arguments and executed directly (`Command`), never interpreted by a shell; no Tauri
  shell plugin, no extra capability. The setting is the user's (encrypted vault);
  `{file}` is the only substituted parameter.
- **Audit**: `edit_open` (session), `edit_save` / `edit_conflict` / `edit_external` —
  never a path or content.
- **Reduced surface**: removing the xterm engine removed the webview-creation capability
  and the `detach-*` windows; the IPC command list is shorter.

### 7.3 Tunnels, multi-exec, macros, logs, imports (M7)

- **Tunnels**: **loopback bind by default**; a non-loopback address is flagged but allowed
  (explicit user choice); SOCKS5 without authentication so NEVER expose it beyond
  loopback; caps (256 connections, timeouts) against resource exhaustion; no payload
  logged; audit `tunnel_start`/`tunnel_stop` without destination.
- **Multi-exec**: a destructive action by nature (one command goes to N sessions) →
  confirmation on enable, permanent native indicator + banner, automatic disable below 2
  members.
- **Macros**: content encrypted in the vault (secret field), shown only on an explicit
  audited action; sending with exact preview (anti-pastejacking) when "Confirm before
  sending" is checked; the startup snippet never runs if it requires confirmation.
- **Session logs** (opt-in per connection): **encrypted by default** (`.log.enc`,
  XChaCha20-Poly1305 in authenticated blocks, SHA-256 subkey derived from the DEK;
  readable in Settings with the vault unlocked; "Encrypt session logs" checkbox can be
  unchecked → plaintext `.log`), configurable **retention** (30 days by default, purge on
  every unlock + button), 0600 files in a 0700 directory, escape sequences stripped,
  50 MiB cap; **acknowledged limit**: an echoed password may appear there (warning in the
  editor); OSC 52 payloads (clipboard) never enter it.
- **Imports**: preview and keys stay in the backend; bounded key reading (64 KB, PRIVATE
  KEY header); hostile entries bounded (4 MiB, 16 KB/line, Include depth 8); never
  modifies existing data; Remmina/MobaXterm passwords **not decrypted** (re-entry) — no
  decryption of third-party secrets.

### 7.4 Vault integrity (hardening)

- Atomic write (tmp 0600 → fsync → rename → **parent directory fsync**) then
  **verification re-read**.
- Rotation of the 5 backups **only if the current vault is structurally valid**: a
  corrupted file can never evict a good backup.
- On unlock, an unreadable vault triggers the backup display (marked valid/corrupted);
  restoring a corrupted backup is refused.

## 8. Acknowledged limits (honesty)

1. **Compromised workstation with an open session (T2)**: malware running as the user can
   read process memory, intercept IPC, capture the keyboard. No vault manager solves this
   case; auto-lock shrinks the window.
2. **Clipboard history**: copying a secret carries the "secret" indicator
   (`x-kde-passwordManagerHint`, like password managers) which Klipper honors: the value
   does not enter the history and is not restored after clearing. Managers that ignore
   this indicator may still keep the value. The clipboard is cleared as soon as the vault
   locks (if its content is still the last copied secret), otherwise when the delay
   expires. The UI reminds you of this limit when using "Copy" on a secret.
3. **Session logging**: terminal output may contain echoed secrets; this is inherent to a
   terminal. Opt-in, warned, 0600.
4. **Core dumps / swap**: the DEK may end up in a dump or swap if the OS allows them.
   Documented recommendations (encrypted swap, `ulimit -c 0`); the app does not disable
   system dumps (out of its scope, the user's machine).
5. **Network metadata**: SSH connections reveal IPs/ports/hosts to a network observer
   (the vault encrypts at rest, not traffic).
6. **Weak export passphrase**: bundle security = export passphrase strength + Argon2id.
   The UI shows a strength indicator and refuses empty passphrases; no arbitrary minimum
   length imposed beyond 8 characters (warning below).
7. **No protection against a root attacker**: root can do everything (memory, disk,
   keyboard). Out of model.
8. **RNG**: we trust `getrandom`/OsRng (kernel). No HSM/TPM in v1.
9. **Forgotten master password = lost data**: no backdoor, no "recovery key" stored
   elsewhere. That is the contract. (Post-v1 idea: optional printable recovery phrase,
   additional DEK wrap.)

## 9. Audit log (local, no secrets)

`audit.log` (JSON lines, 0600, rotation): timestamp, event, target id, result. Events:
`vault_unlock` (method master/keyring), `vault_lock` (reason), `master_change`,
`keyring_cache_on/off`, `credential_reveal`, `clipboard_copy_secret`, `export` (scope,
includes_secrets), `import` (source, strategy, counters), `hostkey_tofu_accept`,
`hostkey_change_rejected/replaced`, `session_log_enabled`, `external_editor_launch`.
Viewable in the UI (Settings → Audit log (no secrets)).

## 10. Vulnerability reporting

Personal project under construction: no bug bounty program. Eventually a security contact
will be published in this file (placeholder: `security@<project domain>` — **do not
publish a personal address**). A 90-day coordinated disclosure policy is considered for
the public v1.