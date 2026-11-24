# ARCHITECTURE — HarakaTS v1

> Phase 0 — design document. To be validated before any code is written.
> Versions and APIs verified on 2026-11-20 (sources at the end of the document).

## 1. Goals

A Linux remote connection manager (SSH + SFTP only in v1) combining:

- **Connection tree**: folder tree, property inheritance, decoupled credentials;
- **MobaXterm**: tabbed terminal, session-linked SFTP panel, multi-exec, tunnels;
- **Remmina**: configuration imports — **without its major flaw**: here secrets are
  encrypted in a portable vault, independent of the machine's keyring.

**Acceptance criterion #1**: export on profile A → import on a virgin profile B (different
data directory, different master password) → every connection opens with its credentials.
Automated CI test at every milestone.

## 2. Technical choices and rationale

### 2.1 Frontend: Svelte 5 (runes) — justified choice

| Criterion | Svelte 5 | React 19 |
|---|---|---|
| Runtime weight | ~2–5 KB, compiled | ~45 KB + ecosystem |
| High-frequency updates (terminal, transfers) | fine-grained signal reactivity, no VDOM | re-renders + manual memoization |
| Imperative widget integration (CodeMirror; native VTE terminal outside the webview) | simple: effects + runes, no composite lifecycle | possible but more boilerplate |
| Strict TypeScript | native support | native |

A terminal app spends its time pushing bytes into imperative widgets (CodeMirror; the
terminal itself is native GTK+VTE outside the webview since M3-VTE): the Svelte compiler
and its granular reactivity fit this profile better than a VDOM.
**Decision: Svelte 5 + Vite + strict TypeScript.**

### 2.2 Persistence: encrypted versioned JSON files — justified choice

**Decision: a single vault file** (`vault.hrk`) containing an encrypted versioned JSON
store, atomic writes (tmp + rename), rotating backups. No SQLite in v1.

- Portability is criterion #1: a single self-describing file (header + encrypted payload)
  is trivially exportable, copyable, versionable. SQLite would add an engine, WAL/journal
  files and merge complexity with no benefit at v1 scale (a few hundred connections, no
  relational queries).
- The internal format is canonical JSON (`serde_json`): auditable, diffable after
  decryption, schema migratable via `schema_version`.
- If volume grows (session history, thousands of entries), SQLite can be introduced behind
  the `Store` interface without changing the export format.

### 2.3 keyring 3.6.2 (not 4.x)

`keyring` v4 reorganizes the API (`keyring-core` + store crates). v3.6.2 is stable,
documented, and sufficient: `Entry::new(service, user)`, `set_secret`/`get_secret`,
features `sync-secret-service` + `crypto-rust` + `vendored` (libdbus statically linked →
the .deb/.rpm packages do not require libdbus on the user's machine). v4 migration possible
post-v1.
**User requirement: without KWallet/Secret Service the app works — the keyring cache is
silently disabled, the master password remains the only unlock path.**

### 2.4 rand_core alignment (pitfall avoided)

`chacha20poly1305 0.11` → `aead 0.6` → **rand_core 0.10**; `rand 0.10.4` also uses
rand_core 0.10. **rand 0.10 removed `OsRng`** in favor of `SysRng` (feature `sys_rng`,
trait `TryRng::try_fill_bytes`) — same system source, fallible API. All cryptographic
randomness (DEK, salts, nonces) comes from `SysRng`; nonces are built with
`XNonce::from_slice` (24 bytes) without going through aead's `Generate` trait, which
avoids any trait-bound question. `argon2 0.6` is used via the raw API
(`hash_password_into`, we provide the salt). **No mixing of rand_core 0.6/0.9/0.10** —
documented so we do not fall back into it during updates (the old argon2 0.5 / aead 0.5 /
rand 0.8 versions created two incompatible lineages).

### 2.5 Pinned versions (verified on 2026-11-21 against Cargo.lock / ui/package.json)

**Rust** (MSRV: 1.90, edition 2024, workspace resolver 2):

| Crate | Version | Role |
|---|---|---|
| tauri | 2.12.x | application shell, IPC, deb/rpm bundling |
| russh | 0.65.2 | SSH client (tokio) |
| russh-sftp | 3.0.1 | SFTP v3 client over a russh channel |
| argon2 | 0.6.0 | Argon2id KDF (raw API `hash_password_into`) |
| chacha20poly1305 | 0.11.0 | XChaCha20-Poly1305 (aead 0.6) |
| rand | 0.10.4 | SysRng (OsRng successor, rand_core 0.10) |
| secrecy | 0.10.3 | `SecretString`/`SecretBox` (redacted Debug) |
| zeroize | 1.9.2 | memory clearing |
| keyring | 3.6.3 | Secret Service cache (features above) |
| tokio | 1.x | async runtime |
| serde / serde_json | 1.x | versioned JSON store |
| uuid | 1.x (v4) | identifiers |
| thiserror | 2.x | typed errors |
| tracing / tracing-subscriber | 0.1 / 0.3 | backend logs (secrets redacted) |
| zbus | 6.4.0 | logind / ScreenSaver signals (auto-lock) |
| arboard | 3.7.0 | clipboard + Linux primary selection |
| notify | 8.2.2 | local file watching (remote editing M6) |
| encoding_rs | 0.8.x | non-UTF-8 terminal encodings |
| sha2 / hmac / sha1 | 0.10 / 0.12 / 0.10 | host key fingerprints, hashed known_hosts, integrity |
| base64 | 0.22 | header/format encodings |
| rfd | 0.18.0 | native file dialogs on the backend side (**xdg-portal** backend + wayland, sync via pollster; especially not the `gtk3` feature: gtk-sys 0.19 would conflict on `links = "gtk-3"` with Tauri's gtk-rs 0.18) |
| gtk | 0.18.2 | native VTE terminal windows (same gtk-rs lineage as Tauri, unified `links = "gtk-3"`) |
| libvte-2.91 (FFI) | ≥ 0.50 (Fedora 45 host: 0.84.1; bookworm: 0.70) | Remmina-style native terminal engine; pkg-config probe in build.rs; runtime RPM `vte291`, DEB `libvte-2.91-0` |

**Frontend** (Node 22 LTS, pnpm via corepack):

| Package | Version |
|---|---|
| svelte | 5.58.0 |
| @codemirror/* (state, view, commands, language, language-data, search, merge) | 6.x pinned (view 6.43.15, state 6.7.6, merge 6.12.3) — remote editing M6, lazy-loaded |
| vite | 8.3.8 |
| @sveltejs/vite-plugin-svelte | 7.3.1 |
| typescript | ~5.9 (strict) |
| svelte-check | 4.7.8 |
| eslint / typescript-eslint | ^9 / ^8 |
| vitest | 5.1.1 |
| @tauri-apps/api / @tauri-apps/cli | 2.12.5 / 2.12.6 |
| fuzzysort | 4.0.2 — fuzzy search Ctrl+K |

**Terminal: xterm.js removed in M6** (user request) — native VTE is the **single engine**
(§8); the `@xterm/*` packages are no longer part of the project (UI bundle 580 → 316 KB).
OSC 52: **backend** interception (§9), no renderer-side handler. History of the
xterm/WebKitGTK pitfalls: project memory `xtermjs_webkitgtk_gel` (do not reintroduce xterm).

## 3. Workspace structure

```
HarakaTS/
├── Cargo.toml                  # workspace
├── crates/
│   ├── haraka-core/            # NO Tauri/GUI dependency → headless-testable in CI
│   │   ├── src/vault/          # vault, DEK/KEK, Argon2id, AEAD, AAD, KDF calibration
│   │   ├── src/model/          # Folder, Connection, Credential, Macro, Settings, inheritance
│   │   ├── src/store/          # atomic read/write, backups, schema migrations
│   │   ├── src/export/         # export/import bundle, UUID merge, conflict strategies
│   │   ├── src/importers/      # ssh_config, remmina, mobaxterm
│   │   ├── src/knownhosts/     # internal known_hosts + system import (hashed entries)
│   │   └── src/audit/          # local audit log without secrets
│   └── haraka-ssh/             # SSH session backend (depends on haraka-core)
│       ├── src/session/        # SSH SessionBackend, auth, host keys, jump hosts
│       ├── src/tunnels/        # L / R / D (minimal SOCKS5)
│       ├── src/sftp/           # browser + transfer engine
│       └── src/edit/           # remote editing (temp, watch, conflicts, atomic upload)
├── src-tauri/                  # thin IPC layer: commands, events, capabilities
├── ui/                         # Svelte 5 + Vite + strict TS
│   └── src/lib/{tree,terminal,sftp,editor,dialogs,palette,state,i18n}/
├── tests/                      # import fixtures, test SSH server, CI scenarios
├── packaging/                  # dev Containerfile, bundling configs, CI
└── docs/                       # (the root .md files are authoritative; docs/ = future annexes)
```

Dependency rule: `ui → src-tauri → haraka-ssh → haraka-core`. `haraka-core` knows neither
Tauri nor russh: the A→B export/import test runs without GUI or network.

## 4. Data model

The whole store lives **in the vault's encrypted payload** (see SECURITY.en.md §2 for the
double encryption layer). Internal schema `schema_version: 1`.

```jsonc
{
  "schema_version": 1,
  "vault_id": "uuid-v4",
  "created_at": "RFC3339", "updated_at": "RFC3339",
  "settings": {
    "autolock_minutes": 15,            // 0 = disabled
    "lock_on_suspend": true, "lock_on_session_lock": true,
    "disconnect_on_lock": true,
    "keyring_cache": false,            // opt-in, never required (default: disabled)
    "clipboard_default": "bidirectional",
    "osc52_max_bytes": 100000,
    "paste_confirm_multiline": true,
    "copy_on_select": false,
    "clipboard_clear_seconds": 30,
    "kdf_target_ms": [500, 1000],
    "locale": "fr", "theme": "system",
    "external_editor": null,           // configurable command, backend execution
    "transfer_parallelism": 3,
    "reconnect": { "enabled": true, "max_attempts": 5, "backoff_max_s": 30 }
  },
  "folders": [{
    "id": "uuid", "parent_id": "uuid|null", "name": "Prod",
    "color": "#rrggbb", "icon": "server", "sort_order": 0, "tags": ["prod"],
    "defaults": {                      // inheritance (see §4.1)
      "user": "deploy", "port": 22, "credential_id": "uuid|null",
      "jump_chain": [], "clipboard_policy": null, "keepalive_s": 30,
      "encoding": "utf-8", "auth_method_order": ["agent","publickey","password","keyboard-interactive"]
    }
  }],
  "connections": [{
    "id": "uuid", "folder_id": "uuid", "name": "web-01",
    "protocol": "ssh",                 // only v1 protocol; extensible
    "host": "web-01.example.net", "port": null, "user": null,   // null = inherits
    "credential_id": null,             // null = inherits from the folder
    "jump_chain": [ { "host": "bastion", "port": 22, "user": "jump", "credential_id": "uuid" } ],
    "clipboard_policy": null,          // null = inherits; otherwise disabled|l2r|r2l|bidirectional
    "keepalive_s": null, "encoding": null,
    "color": null, "icon": null, "tags": [], "favorite": false,
    "tunnels": [ { "id":"uuid", "kind":"local|remote|dynamic",
                   "bind_addr":"127.0.0.1", "bind_port":8080,
                   "dest_host":"db.internal", "dest_port":5432, "auto_start":true } ],
    "startup_snippet_id": null,
    "session_logging": false,          // opt-in per connection (see SECURITY.en.md §7)
    "sftp": { "follow_terminal_cwd": false, "show_hidden": false },
    "notes": "", "created_at": "…", "updated_at": "…", "last_used_at": null
  }],
  "credentials": [{
    "id": "uuid", "name": "deploy-prod",
    "username": "deploy",              // not secret (metadata)
    "password":   { "alg":"XChaCha20-Poly1305", "nonce":"b64", "ct":"b64" },  // EncSecret|null
    "private_key": { … },              // full PEM/OpenSSH content, encrypted
    "passphrase":  { … },              // key passphrase, encrypted
    "key_origin": "imported|generated|pasted",
    "needs_reentry": ["password"],     // fields marked "to re-enter" (imports)
    "created_at": "…", "updated_at": "…"
  }],
  "macros": [{ "id":"uuid", "name":"restart-nginx",
               "content": { …EncSecret… },   // encrypted: a snippet may contain a secret
               "confirm_before_send": true }],
  "recents": [ { "connection_id":"uuid", "last_used_at":"…" } ],   // capped at 50
  "ui_state": { "expanded_folders": ["uuid"], "tab_layout": { … } }
}
```

`EncSecret`: random 24-byte nonce + XChaCha20-Poly1305 ciphertext (16-byte tag included),
**AAD = vault_id ∥ entry_id ∥ field_name** (canonical length-prefixed encoding, see
EXPORT_FORMAT.en.md §4). A secret therefore cannot be moved across entries/fields.

### 4.1 Property inheritance

Resolution is **purely backend-side**, at connection time:

```
config_effective = fold(defaults_système, defaults_dossier_racine → … → dossier_parent, overrides_connexion)
```

- A `null` field on the connection = inherits; a value = overrides.
- Folders form a `parent_id` chain (free depth, cycles forbidden — checked on write).
- `credential_id` resolves the same way; the Credential object is shared: editing it once
  updates every connection that references it.
- The clipboard policy is inheritable folder → connection (prompt §3 requirement).
- The UI shows the effective value with an "inherited from <folder>" badge and an
  "override" button.

### 4.2 References and integrity

- All references are UUIDs; deleting a referenced credential → warning + list of impacted
  connections, choice of: detach (connections fall back to inheritance) or cancel.
- `needs_reentry`: a connection whose credential has missing fields opens in
  "authentication to complete" mode (password/passphrase prompt), never crashing or
  blocking the import.

## 5. Vault and crypto (summary — detail in SECURITY.en.md and EXPORT_FORMAT.en.md)

- **DEK**: random 256 bits (SysRng) → encrypts the store payload **and** each secret
  individually (double layer, see SECURITY.en.md §2).
- **KEK** = Argon2id(master password, salt, m, t, p) → wraps the DEK in the header.
- KDF parameters **calibrated at creation** for 0.5–1 s on the machine (m = 128 MiB,
  p = 4, t adjusted; stored in the header, re-calibratable in settings).
- Master password change = **re-wrap of the DEK only** (new salt), the payload is not
  re-encrypted.
- `vault.hrk` file: versioned binary envelope — immutable JSON header + KDF/wrapped-DEK
  block (mutable, authenticated by the wrap tag) + AEAD payload. A master change rewrites
  only the KDF/wrap block (payload unchanged, O(1)).
  Detailed binary format: EXPORT_FORMAT.en.md (amendment of 2026-11-20).
- Atomic writes: `vault.hrk.tmp` (0600) → fsync → rename; `backups/` keeps the last 5
  versions; corrupted payload → restore-from-backup proposal.
- **OS keyring (optional, non-blocking)**: if `keyring_cache = true` and a Secret Service
  answers (KWallet on KDE), the DEK is deposited there (`service="harakats"`,
  `user=vault_id`, `set_secret`) to unlock without a password on this machine. Service
  failure/absence → silent fallback to the master password. The DEK wrapped by the master
  password **always** remains in the header: the keyring is never required (this is the
  fundamental difference from Remmina).
- **Auto-lock**: inactivity timer (UI events → backend), logind `PrepareForSleep(true)`
  signal, session lock (`org.freedesktop.ScreenSaver` / logind session, via zbus). On
  lock: `zeroize` DEK + in-memory keys, session disconnect (default, configurable), the UI
  returns to the unlock screen.

## 6. Protocol abstraction

```rust
#[async_trait]
pub trait SessionBackend: Send + Sync {
    fn protocol(&self) -> Protocol;                    // Ssh (v1); Rdp, Vnc, Telnet, Serial later
    fn capabilities(&self) -> Capabilities;            // terminal, filesystem, tunnels, clipboard…
    async fn connect(&mut self, cfg: &EffectiveConfig, secrets: &SecretResolver) -> Result<()>;
    async fn open_terminal(&mut self, size: TermSize) -> Result<TerminalHandle>;
    async fn open_filesystem(&mut self) -> Result<FsHandle>;      // SFTP for SSH
    async fn open_tunnel(&mut self, spec: &TunnelSpec) -> Result<TunnelHandle>;
    async fn disconnect(&mut self) -> Result<()>;
    fn state(&self) -> SessionState;                   // Connecting, AuthPrompt, Connected, Reconnecting, Closed
}
```

- `TerminalHandle`: `write(bytes)`, `resize(size)`, `close()`, output stream via
  `tokio::mpsc` (bounded, coalesced).
- `FsHandle`: async `std::fs`-like interface (list, stat, read, write, rename, remove,
  chmod/chown, mkdir, symlink) — implemented by russh-sftp.
- `SecretResolver`: secret access **backend-side only**, for the duration of auth;
  material is zeroized after use.
- **v1 honesty**: RDP/VNC will have a different rendering surface (image/video, not a text
  terminal). The abstraction covers lifecycle, auth, clipboard and tunnels;
  `capabilities()` will let the UI pick the widget. Adding a protocol will not refactor
  the vault, the tree, the IPC or the transfers — but an RDP backend will need its own
  rendering module (out of v1).
- `Protocol` is a serialized enum; a connection unknown to the schema → clean error, never
  a panic.

## 7. SSH module (`haraka-ssh`)

russh 0.65.2 APIs **verified** (docs.rs):

- `client::connect(config, addr, handler)` / `client::connect_stream(config, stream, handler)`
  (stream = `AsyncRead + AsyncWrite + Unpin + Send`) → basis for jump hosts.
- Auth: `Handle::authenticate_password`, `authenticate_publickey(user, PrivateKeyWithHashAlg)`
  (RSA: `best_supported_rsa_hash` to pick the hash alg), and keyboard-interactive in
  **two steps**: `authenticate_keyboard_interactive_start(user, submethods)` then
  `authenticate_keyboard_interactive_respond(Vec<String>)` (loop while the server returns
  prompts — 2FA/OTP).
- Keys: `russh::keys` (loading/decryption of ed25519/ecdsa/rsa keys, **SSH agent** via
  `SSH_AUTH_SOCK`). Exact symbols (`load_secret_key`, `agent::AgentClient`, `known_hosts`)
  re-verified at M3; fallback: minimal implementation of the agent protocol (it is simple
  and stable) if the API moved.
- Host keys: `Handler::check_server_key(&PublicKeyOrCertificate)` — the default impl
  **rejects everything**, it is overridden:
  1. SHA256 fingerprint (OpenSSH format `SHA256:base64-no-padding`);
  2. internal known_hosts (`data/known_hosts`, OpenSSH format, 0600) + import of the
     system `~/.ssh/known_hosts` (hashed entries `|1|salt|hash` supported: HMAC-SHA1);
  3. unknown → **TOFU**: blocking dialog with fingerprint, "Accept and remember" /
     "Reject";
  4. **changed key → blocking red alert**, old/new fingerprints displayed, refusal by
     default; replacement only on explicit action, logged (audit).
  The handler waits for the UI answer via a `oneshot` — no automatic decision.
- Channels: `channel_open_session()` → `request_pty(term, cols, rows, …)` →
  `request_shell(true)`; resize = **`window_change`** (not `resize_pty`, which does not
  exist); writing = `data_bytes` / `make_writer`; output = `Handler::data` → mpsc.
  **Imperative russh rule**: russh delivers every channel message to `Handler::data` AND to
  the `Channel`'s internal queue (bounded, `channel_buffer_size` = 100). This queue must be
  **drained**: the shell channel is `split()`; the read half is consumed by a task
  (`read_half.wait()`), the write half (`ChannelWriteHalf`) serves `data_bytes` /
  `window_change`. Without draining, the session loop blocks at the ~100th message (htop
  freeze). The `inner` lock is never held across a network `await` (handle cloned then
  released).
- **Chained jump hosts**: TCP → russh(hop1) → auth → `channel_open_direct_tcpip(hop2)`
  → `into_stream()` → `connect_stream`(hop2) → … → target. Each hop has its credential
  (inheritance included). `ProxyJump` imported from ssh_config maps directly to this chain.
  `ProxyCommand`: **deferred post-v1** (requires a shell spawn, unnecessary risk surface).
- **Tunnels**:
  - *Local (L)*: tokio listener on `bind_addr:port` → per incoming connection
    `channel_open_direct_tcpip(dest)` → bidirectional pump;
  - *Remote (R)*: `Handle::tcpip_forward(addr, port)` (port 0 = chosen by the server,
    returned) → `Handler::server_channel_open_forwarded_tcpip` → local connection to the
    target → pump; depends on server-side `GatewayPorts` (documented in the UI);
  - *Dynamic (D)*: **minimal SOCKS5** server (RFC 1928: no-auth + CONNECT,
    IPv4/IPv6/domain; no UDP ASSOCIATE or BIND — documented) → `direct-tcpip`.
  - Graphical manager: per-connection specs + live tunnels (hot add/remove, state,
    counters, auto_start).
  - *M7 implementation* (`haraka-ssh::tunnel`, `src-tauri/src/tunnels.rs`): per-session
    `TunnelManager` (states `starting|listening|error`, counters refreshed ≤ 1/s,
    `tunnel://state` event), remote-forward registry shared with `SshHandler`, idempotent
    auto-start via `post_connect.rs`, stop on session close. Multi-exec:
    `haraka-ssh::multiexec` (`write_routed` writes to the origin then to the other members
    via `SessionManager::write`, never through their input queue → no loop). Session log:
    `haraka-core::sessionlog` plugged into the output forwarder after OSC 52.
- Keepalive: `send_keepalive(want_reply=false)` task at a configurable interval;
  `send_ping` to measure latency (shown in the status bar).
- Auto-reconnect: backoff 1→2→4→…→30 s, max 5 attempts (adjustable), re-auth from the
  backend memory cache (only while the vault is unlocked); final failure → tab in
  "disconnected" state, reconnect button. On vault lock: no reconnection.
- Encoding: UTF-8 by default, `encoding_rs` for legacy charsets configurable per connection
  (inheritable).
- Agent forwarding: **disabled, not offered in v1** (risk of the remote server abusing the
  agent socket; security decision documented in SECURITY.en.md).

## 8. Terminal and UI

- **Two engines (M3-VTE, user decision of 2026-11-20)**:
  - **Native VTE (default, `settings.terminal_engine = "vte"`)** — Remmina's engine.
    GTK+VTE window **per session** (native widget, system rendering/keyboard/resize/
    copy-paste). Direct FFI on **libvte-2.91** (stable C API, LGPL): the existing Rust
    bindings (`vte-rs`) are GPL-3.0 or based on a GTK incompatible with Tauri.
    "Feed" mode without a local PTY: decoded SSH output → `vte_terminal_feed`; keyboard →
    "commit" signal; "size-allocate" → `window_change`. Guarantees (ameliorations.md
    review, 2026-11-20): dimensions read in **idle** (VTE grid up to date, never the old
    size); **one ordered task per session** (mpsc Input/Resize, local encoder — no
    input/resize reordering); russh write/resize **outside the global lock** (Arc<Channel>
    cloned under lock, network await outside); **coalesced** output (a single GTK invoke
    in flight, bounded 16 MiB buffer); resize debounced 100 ms. Native status bar
    (state/latency/dims/Reconnect), context menu (native VTE copy/paste, selection, reset),
    errors visible in the terminal. All GTK calls on the main thread (glib MainContext),
    never from tokio. Auth/host-key prompts = native GTK dialogs (same TOFU policy;
    Cancel = empty answer → clean rejection).
    Reason for the pivot: xterm.js inside WebKitGTK froze htop/tmux (unstable WebGL
    renderer + IPC flood) despite the DOM renderer and batching — the backend was healthy,
    the webview rendering was at fault. VTE takes the terminal out of the webview.
  - **Single engine**: the former xterm.js fallback (tabs, splits, detachment) was removed
    (M6); the Tauri capability `core:webview:allow-create-webview-window` and the `detach-*`
    window with it.
- Sessions: one GTK+VTE window per session (the backend keeps session state), **or** a tab
  of a `gtk::Notebook` placed above the webview in the main window (`terminal_in_tabs`
  option, `vte/tabs.rs` module: the webview is reparented into a `gtk::Paned` on the first
  tab opening; `VteWindow.page_ptr != 0` = tab mode, closing via `handle::close_tab`).
  Session bar in the main window (`sessions.svelte.ts` store, 2 s poll +
  `session://opened|closed|active` events emitted by `sessions.rs` / GTK focus); the SFTP
  panel follows the active session.
- **Output highlighting**: `haraka-core::highlight::Highlighter` (fixed rules, state machine
  that skips escape sequences, tracks the alternate screen and the foreground color set by
  the server), applied in `spawn_output_forwarder` AFTER logging (raw log). Setting
  `terminal_highlight`.
- Data flow: `Handler::data` → bounded mpsc → **`tauri::ipc::Channel<Vec<u8>>`**
  (`serde_bytes` serialization, no base64) → `term.write` batched by
  `requestAnimationFrame`. Input: `session_write(session_id, bytes)` (invoke).
  Backpressure: coalescing of small packets, bounded channel (SSH flow control does the
  rest).
- **Multi-exec** (non-modal floating panel on the UI side): tab/panel selection → every
  keystroke is duplicated to the selected sessions (backend fan-out), permanent visual
  indicator "MULTI-EXEC" + confirmation on enable.
- Macros/snippets: content encrypted in the vault, execution = sending bytes to the chosen
  sessions, configurable confirmation. No secret interpolation in macros in v1 (avoid
  involuntary echo; documented).
- Quick-connect bar `user@host:port` (on-the-fly creation, save proposal), favorites,
  recents, **fuzzy search Ctrl+K** (fuzzysort: connections, folders, credentials, actions).
- **Optional per-connection session logging**: raw terminal stream to
  `data/logs/<session>.log` (0600). UI warning: terminal output may contain echoed secrets
  (passwords typed in clear text on the shell side); no backend auth material is ever
  written there (see SECURITY.en.md §7).
- i18n: JSON catalogs FR (default) + EN, structure ready from M2.

## 9. Clipboard (per-connection policies, inheritable)

Policy: `disabled | local→remote | remote→local | bidirectional` (system default
`bidirectional`, folder then connection override).

| Direction | Mechanism | Control |
|---|---|---|
| local → remote | terminal paste (Ctrl+Shift+V, middle click, bracketed paste) | multiline/suspicious confirmation (`paste_confirm_multiline`); explicit user action |
| remote → local (write) | **OSC 52 intercepted in the backend** (`haraka_core::osc52::Osc52Extractor`, output pipeline — sequences never reach the terminal) | `r2l`/`bidirectional` policy + `osc52_max_bytes` (default 100,000 bytes), audited |
| remote → local (query) | OSC 52 `?`: the remote **reads** the local clipboard (potential exfiltration) | `l2r`/`bidirectional` policy, **always audited**, response via `query_response` |
| local copy | Ctrl+Shift+C, **copy-on-select** (option) → clipboard + primary selection (arboard, Linux extensions) | local policy |
| files | copy-paste between the local panel and the remote SFTP panel (internal format `application/x-harakats-files`) | **M5** (depends on the SFTP panel; documented M4 deviation) |
| OS → app drag & drop | Tauri v2 drag-drop events (file paths) → SFTP upload | **M5**; never loads a remote URL |

M4 implementation (2026-11-20): streaming extractor with inter-chunk carry (sequences split
across packets), BEL and ST terminators, other OSCs intact; inherited effective policy
captured at open (`ClipCtx` per session). Paste confirmation: `haraka_core::paste` (Rust,
GTK dialog with ␛␍␊ preview) — bracketed-paste markers stripped **at the edges only**
(internal marker = pastejacking → confirmation). Since M6 the engine is VTE-only: the
TypeScript mirror `paste.ts` and the xterm path were removed; VTE paste goes through
`paste_from` (single confirmation, never on the `commit` signal).

- **Bracketed paste** always active on the remote terminal side (the remote shell decides).
- **Anti-malicious-paste**: before any paste, content analysis; confirmation if multiline
  (adjustable) or if suspicious control characters/escapes (beyond `\n`, `\t`), with an
  exact preview of what will be sent.
- Copying a secret (explicit action): `clipboard_copy_secret` → backend clipboard write →
  **automatic clearing after N seconds** (default 30, adjustable); the secret is never
  stored on the frontend side. Acknowledged limit: clipboard history managers (Klipper…)
  may keep a trace — documented in SECURITY.en.md.
- **VTE code** (`src-tauri/src/vte/`): `ffi` (the only `unsafe`, `Term` wrapper), `handle`
  (`Send+Sync` handle + registry, return to the GTK thread via `on_main_if_alive`),
  `window` (assembly), `clipboard`, `resize`, `dialogs`, `mod` (`VteCallbacks`,
  `VteOptions`, `open_window`).
- **Native VTE paste**: confirmation is NEVER on the `commit` signal (keystrokes, key
  sequences, fragmented bracketed-paste markers). Explicit paste is read on the GTK side
  (`paste_from`, CLIPBOARD or PRIMARY), confirmed once if suspicious, then sent via
  `SessionCmd::Paste` (wrapped `ESC[200~…ESC[201~` if the terminal enabled DECSET 2004,
  tracked in the output stream). Ctrl+Shift+C/V, Ctrl/Shift+Insert are wired by the
  application.
- **Acknowledged v1 limit**: drag-out (dragging a remote file to the OS file manager) is
  not supported by Tauri v2 webviews; replaced by "Copy to…" / download. OS → app drag-in
  works.

## 10. SFTP and transfers

> **M5 status: implemented** (`crates/haraka-ssh/src/{sftp,transfer,osc7}.rs`, `src-tauri/src/sftp.rs`,
> `ui/.../SftpPanel.svelte`). Effective design points:
> - `SessionManager::sftp(session)`: single-flight opening of a 2nd channel (`Arc<Handle>` cloned
>   outside the global lock), cached in `SessionEntry`, `sftp_invalidate` on fatal error.
> - **`SshHandler::data` feeds the terminal only for the shell channel** (`shell_channel`);
>   otherwise SFTP bytes would be displayed. The SFTP channel (`into_stream`) is read continuously
>   by `SftpSession`: no russh queue to drain.
> - Resume: `.part` + comparison of the trailing 64 KiB on both sides (no sidecar); final `rename`
>   (SFTP v3 does not overwrite → prior deletion if "replace").
> - OpenSSH reads `SSH_FXP_SYMLINK` as (target, link) — `sftp::symlink` swaps russh-sftp's arguments.
> - Terminal following: `sftp::FOLLOW_HOOK` (fixed bash/zsh line, idempotent) typed via
>   `SessionManager::write` by `sftp_enable_follow` or, if `settings.sftp_follow_terminal`, on every
>   `connected` state of a VTE session (`forward_native`).
> - Hook-line concealment: `vte::begin_conceal` (SGR 8 + echo line-count computation)
>   → `ClipCtx.hook_rows` → the output forwarder inserts `ESC[0m ESC[nA CR ESC[J` right after the first
>   OSC 7 (`Osc7Scanner::first_end`); 4 s guard timer.
> - SFTP: non-modal integrated Dashboard view (`SftpPanel` mounted once, `hidden` toggles).
> - Main window: `window_state.rs` (window created invisible, size restored then `show`, saved on
>   `CloseRequested`/`Destroyed`, "normal" size tracked while not maximized).
> - UI events: `transfer://event` (`TransferInfo` snapshot, ≤ 5/s per job), `sftp://cwd`
>   (OSC 7), `sftp://open` (VTE window button).

- **Same SSH connection**: `channel_open_session()` → `request_subsystem("sftp")` →
  `channel.into_stream()` → `russh_sftp::client::SftpSession::new(stream)` (verified API:
  `new<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>`). No second auth.
- Side panel (toggle): tree + list, hidden files, sorting, search, rename, delete, mkdir,
  chmod/chown (`set_metadata`), properties, copy path, symlinks (`symlink`, `read_link`).
- "Follow the terminal's directory": **OSC 7** (`file://host/path`) emitted by the remote
  shell; the app may type a fixed line (PROMPT_COMMAND/precmd) into the session shell on
  explicit action (or opt-in setting) — **never written to remote files**; a paste-ready
  snippet is available for permanent following.
- **Transfer engine**:
  - persistent in-memory queue, parallel workers (default 3, max 8);
  - progress, throughput, ETA via `Channel<TransferProgress>`;
  - pause/resume (cancellation token + preserved offset); **interrupted-transfer resume**:
    local `.part` file + offset, remote size ≥ offset check, repositioning (`AsyncSeek` on
    russh-sftp `File` — confirmed at M5);
  - recursive folders (`read_dir` traversal, symlink policy: ignore by default);
  - permission + date preservation (`set_metadata` after completion);
  - integrity: size always; SHA-256 hash if the SFTP `check-file` extension is offered by
    the server (rare); otherwise a "full re-verification" option (remote re-read, costly,
    opt-in). **Honesty**: SFTP v3 has no standard hash — this is documented in the UI.
  - atomic upload for remote editing: remote `.harakats-<uuid>.tmp` write + `rename`
    (see §11).

## 11. Remote editing

> **M6 status: implemented** (`crates/haraka-ssh/src/edit.rs`, `src-tauri/src/edit.rs`, `ui/.../EditorView.svelte`).
> Deviations / clarifications relative to the plan below:
> - Content is handled as **bytes** on the backend side (CRLF/LF line endings and non-UTF-8 bytes
>   preserved; a non-UTF-8 file is editable only externally). Editing limit: 20 MiB.
> - **Remote replacement**: SFTP v3 does not overwrite via `rename` and `russh-sftp` does not expose
>   `posix-rename@openssh.com` → remote temporary `.<name>.haraka-<id>.tmp` (same folder, copied rights),
>   then target renamed to `.bak`, temporary renamed, `.bak` deleted; on failure the target is
>   RESTORED. Never a partial file under its name; a short window without a file during replacement
>   (a concurrent reader sees the old content, the new one, or a brief absence).
> - Conflict: `mtime/size` then SHA-256 (a `touch` is not a conflict); deleted file = "no longer
>   exists" conflict (overwriting recreates it).
> - External editor: command in **argv**, shell-like splitting but WITHOUT a shell (`{file}` or path
>   appended), empty = `xdg-open`; `notify` watching of the temp folder + 400 ms debounce; the echo of
>   our own write is ignored (hash comparison).
> - Cleanup: document close, vault lock (`close_all`), startup purge.

1. Open: download to `data/tmp/<uuid>/<filename>` (directory **0700**, files 0600),
   recording `mtime + size + SHA-256 hash` of the remote file at time T.
2. Edit: integrated CodeMirror 6 (extension-based highlighting, search, LF/CRLF line
   endings, tabs) **or** configurable external editor (command launched by the backend,
   never through a Tauri shell plugin).
3. Save: `notify` watcher on the local file → on every stable write (300 ms debounce):
   re-stat the remote; if `mtime/size` changed → download the remote into a second temp,
   SHA-256 comparison:
   - identical → silent upload;
   - different → **conflict dialog**: overwrite / reload / **compare** (`@codemirror/merge`
     diff view).
4. Atomic upload: remote temp in the same directory → `set_metadata` → `rename`.
5. Close: cleanup of the temp directory (and on app exit, including crash: startup purge).

## 12. Imports

> **M7 status: implemented** — `crates/haraka-core/src/import/{mod,ssh_config,remmina,mobaxterm}.rs`
> (pure parsers + planner + applicator over the `Store`), `src-tauri/src/import_external.rs`
> (file dialog, key reading, **preview kept backend-side**: 8 previews max, only structured
> counters/names/warnings `{code, params}` go to the UI), applicator: "Import <source>" group,
> conflicting names suffixed, keys encrypted via `vault.set_secret`. Bounds: 4 MiB entry, 16 KB lines,
> 5000 connections, 200 warnings, Include depth 8 / 200 files.

| Source | Method | Secrets |
|---|---|---|
| `~/.ssh/config` | tested homegrown parser (Host patterns + negation, `Include` glob, User, HostName, Port, IdentityFile, IdentitiesOnly, ProxyJump → hop chain; `Match` and `ProxyCommand` deferred, documented). Wildcard hosts (`*`, `?`) become **folder defaults**, not connections. | IdentityFile → key read and **imported into the vault** (encrypted); unknown passphrase → `needs_reentry` |
| `.remmina` | homegrown INI parser (`protocol=SSH`, server, username, ssh_tunnel_* → tunnels); unknown fields ignored without error | `password=` encrypted by Remmina with its local key → **`needs_reentry` by default**. Experimental M7 option: best-effort decryption if the user provides their `remmina.pref` (their own machine, their choice) |
| `.mxtsessions` (MobaXterm) | homegrown INI-like parser (Bookmarks sections, SubRep → folders, Login/Host/Port) | encrypted passwords (proprietary MobaXterm scheme) → **`needs_reentry`**; decryption not guaranteed, deferred |

Common rule: **an import never crashes** on an unknown/corrupted field; it produces a
report `{ imported, ignored, secrets to re-enter }` and an audit entry. Imported
connections land in a dedicated "Import <source>" folder for review.

## 13. Tauri v2 IPC

Principle: **minimal, typed commands, validated backend-side** (manual zod-like or serde +
guardrails), sanitized DTOs (no secret field except explicit reveal). Secrets flow
**frontend → backend** only when the user types them (credential creation/editing);
**backend → frontend** only via `credential_reveal` / `clipboard_copy_secret` (explicit
action, logged).

### Commands (group → examples)

| Group | Commands |
|---|---|
| Vault | `vault_status`, `vault_create`, `vault_unlock` (master \| keyring), `vault_lock`, `vault_change_master`, `vault_keyring_set_enabled` |
| Tree | `tree_get` (sanitized DTO), `folder_upsert/delete/move`, `connection_upsert/delete/move/duplicate`, `connection_effective_config` (inheritance preview, no secrets) |
| Credentials | `credential_list` (metadata + `needs_reentry`), `credential_upsert` (secrets typed by the user), `credential_delete`, `credential_reveal(id, field)`, `clipboard_copy_secret(id, field, ttl)` |
| Sessions | `session_open(connection_id \| quick_spec, output_channel: Channel<TermEvent>)`, `session_write`, `session_resize`, `session_close`, `session_reconnect`, `multi_exec_set(sessions, enabled)`, `auth_prompt_respond(prompt_id, answers)` |
| SFTP | `sftp_list/stat/mkdir/rename/remove/chmod/chown/readlink/copy_path`, `sftp_follow_cwd_set` |
| Transfers | `transfer_enqueue(specs, progress_channel)`, `transfer_pause/resume/cancel`, `transfer_list` |
| Editing | `edit_open(connection_id, remote_path)` → content + temp path, `edit_conflict_resolve(choice)`, `edit_close` |
| Tunnels | `tunnel_upsert/delete/start/stop`, `tunnel_list` (live state) |
| Export/Import | `export_bundle(scope, passphrase, options)` (native file dialog via rfd, backend), `import_bundle_preview(path, passphrase)`, `import_bundle_apply(preview_id, strategy)`, `import_ssh_config`, `import_remmina`, `import_mobaxterm` |
| Host keys | `known_hosts_list`, `known_hosts_import_system`, `hostkey_prompt_respond(prompt_id, decision)` |
| Misc | `settings_get/set` (non-secret subset), `macro_upsert/delete/run`, `audit_log_list`, `palette_search` |

### Events (backend → UI, sanitized)

`vault://state` (locked/unlocked/keyring_available), `session://state`,
`auth://prompt` (keyboard-interactive questions, banner), `hostkey://prompt`
(TOFU/change fingerprints), `transfer://progress` (also via Channel),
`tunnel://state`, `import://report`, `lock://reason`.

### Capabilities and CSP

- Capabilities: `core:event:default`, permissions of the commands above (declared
  explicitly), `core:webview:allow-create-webview-window` + `core:window:default`
  (detached tabs). **No fs/shell/http/dialog plugin**: files via rfd on the backend side,
  external process (editor) via `std::process` on the backend side only.
- Main window: `devtools` enabled **in dev builds only**.
- Strict CSP (tauri.conf.json):

```
default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self' data:; font-src 'self'; connect-src 'self' ipc: http://ipc.localhost;
object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'
```

`'unsafe-inline'` for styles only: CodeMirror injects dynamic styles (no nonce possible);
no inline script, no remote URL loaded.

## 14. Data directories and portable mode

- Default: `$XDG_DATA_HOME/harakats` (→ `~/.local/share/harakats`), **0700**;
  **0600** files: `vault.hrk`, `backups/vault-<n>.hrk`, `known_hosts`, `audit.log`,
  `logs/` (opt-in sessions), `tmp/` (remote editing, purged at startup).
- **Portable mode** (proposed addition): if a `portable.marker` file exists next to the
  binary (or `--data-dir <path>`), data lives in `./data/` — USB stick or synchronized
  repository. Same format, same crypto: this is the essence of the project.
- No secret outside the vault (no plaintext cache, no "remember password" anywhere other
  than the optional keyring described in §5).

## 15. Tests and CI (per-milestone detail in ROADMAP.en.md)

- **In-process test SSH server**: russh is both client **and server** → a minimal russh
  server in `tests/` (controllable password/key/keyboard-interactive auth, fixed host key,
  shell/sftp/direct-tcpip channels) for hermetic, fast, network-free tests.
- **Real interop**: `openssh-server` started in the dev container (Podman) and on the CI
  runner (port 2222, dedicated config, disposable test keys) for SFTP, tunnels, transfer
  resumes. No docker-in-docker needed.
- `haraka-core` is headless: the **A→B test** (export profile A, import virgin profile B,
  different master password, opening connections with credentials) runs in CI without a
  GUI. Tampering: a modified bit in header/payload/secret → rejection (AEAD tag).
- GitHub Actions CI: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo audit`,
  `cargo test` (workspace), `pnpm lint`, `pnpm check` (strict tsc), `pnpm test` (vitest),
  `.deb` + `.rpm` build. (The git repository and push remain in your hands.)

## 16. Packaging

- `tauri.conf.json`: bundle targets **`deb` + `rpm`** (user decision of 2026-11-20;
  AppImage removed from the v1 scope), icons, `com.harakats.desktop` metadata,
  Network/System categories.
- **.deb** (Debian 12+ / Ubuntu 22.04+): declared runtime dependencies
  (`libwebkit2gtk-4.1-0`, `libgtk-3-0`, `libsoup-3.0-0`, `libjavascriptcoregtk-4.1-0`,
  `libglib2.0-0`) — `apt install ./harakats_*.deb` pulls them automatically.
- **.rpm**: produced natively by the Tauri bundler (`bundle.linux.rpm.depends`, no manual
  rpmbuild step). Targets verified on 2026-11-20: **Fedora ≥ 42** (official webkit2gtk4.1,
  2.48→2.51), **RHEL/Rocky/Alma 10 via EPEL** (webkit2gtk4.1-2.48.3 published), Mageia
  9/10. Declared RPM dependencies: `webkit2gtk4.1`, `javascriptcoregtk4.1`, `gtk3`,
  `libsoup3`, `glib2` — to be confirmed with `rpm -qp --requires` on the package produced
  at M7.
  **EL9 (RHEL/Rocky/Alma 9) NOT supported**: webkit2gtk4.1 absent from the repositories
  and glib2 2.68 < 2.70 required by the soup3 stack (see tauri-apps/tauri#9039).
- Build of **both formats in the same Debian bookworm container** (glibc 2.36, lower than
  all supported targets → forward compatibility).
- Optional GPG signing of .rpm (`TAURI_SIGNING_RPM_KEY` /
  `TAURI_SIGNING_RPM_KEY_PASSPHRASE`, keys never in the repository) — decision at M7.
- keyring `vendored` (libdbus statically linked): no libdbus required on the user's
  machine, whatever the format.
- Flatpak (the runtime embeds the webview → "all-in-one"): **post-v1** option, no impact
  on the architecture.
- Windows/macOS: portable code (no blocking `#[cfg(target_os)]` outside keyring/zbus which
  have native backends), **not shipped in v1**.

## 17. Improvements added to the prompt (proposed)

1. **Encryption of the whole store** (not just secrets): hosts, users and topology are
   sensitive metadata (double layer, SECURITY.en.md §2).
2. **Rotating vault backups** + guided restore on corruption.
3. **Portable mode** (data next to the binary) — aligned with the portability mission.
4. **Local audit log** (unlocks, reveals, copies, exports, host key changes) without any
   secret.
5. **Automatic calibration of Argon2id parameters** at creation (target 0.5–1 s).
6. **In-process test SSH server** (russh server) in addition to the openssh container.
7. **FR/EN i18n** from M2 (FR by default).
8. **Non-blocking keyring**: missing KWallet/Secret Service → silent fallback (explicit
   user requirement).
9. Latency indicator (`send_ping`) and tunnel state in the status bar.

## 18. Acknowledged / deferred v1 limits

- RDP, VNC, Telnet, serial: abstraction ready, backends deferred.
- `ProxyCommand` and `Match` in the ssh_config import (shell spawn / complexity).
- SSH agent forwarding (security risk, documented decision).
- File drag-out to the OS (Tauri v2 webview limitation); SOCKS5 UDP ASSOCIATE / BIND;
  standard SFTP remote hash (`check-file` extension only).
- Decryption of MobaXterm / Remmina passwords on import: `needs_reentry` by default
  (experimental best-effort Remmina in M7).
- Secret interpolation in macros; asciinema-style session recording.
- SSH host/user certificates (`PublicKeyOrCertificate` is ready on the russh side):
  post-v1.
- EL9 (RHEL/Rocky/Alma 9): not supported in v1 (webkit2gtk4.1 absent, glib2 too old for
  the soup3 stack); AppImage removed from scope (user decision of 2026-11-20, in favor of
  .deb + .rpm).
- Windows/macOS not shipped; no telemetry, no automatic update (no remote URL in the
  webview).

## 19. Sources (version/API verification, 2026-11-20)

- https://crates.io/api/v1/crates/russh (0.65.2, 2026-11-16)
- https://docs.rs/russh/latest/russh/client/struct.Handle.html
- https://docs.rs/russh/latest/russh/client/trait.Handler.html
- https://docs.rs/russh/latest/russh/struct.Channel.html
- https://docs.rs/russh/latest/russh/ (keys/agent modules, no root ssh_config/known_hosts)
- https://crates.io/api/v1/crates/russh-sftp (3.0.1, 2026-09-28)
- https://docs.rs/russh-sftp/latest/russh_sftp/client/struct.SftpSession.html
- https://crates.io/api/v1/crates/argon2 (0.6.0) ; https://docs.rs/argon2/latest/argon2/
- https://crates.io/api/v1/crates/chacha20poly1305 (0.11.0) ; https://docs.rs/aead/latest/aead/ (rand_core 0.10, AeadInOut)
- https://crates.io/api/v1/crates/rand (0.10.4) ; https://crates.io/api/v1/crates/secrecy (0.10.3) ; https://crates.io/api/v1/crates/zeroize (1.9.2)
- https://docs.rs/keyring/3.6.3/keyring/ (features sync-secret-service/crypto-rust/vendored)
- https://v2.tauri.app/release/ (2.12.x, MSRV 1.90) ; https://v2.tauri.app/start/prerequisites/
- https://registry.npmjs.org/svelte/latest (5.58.0) ; https://registry.npmjs.org/@codemirror/view/latest (6.43.15)
- https://v2.tauri.app/distribute/rpm/ (native RPM bundling, `bundle.linux.rpm.depends`, build baseline)
- https://pkgs.org/download/webkit2gtk4.1 (Fedora 42–44, EPEL 10, Mageia 9/10; absent on EL9)
- https://github.com/tauri-apps/tauri/issues/9039 (EL9/CentOS 9: glib2 2.68 < gio 2.70 required by soup3)
- https://docs.rs/rand/0.10.4/rand/rngs/struct.SysRng.html (OsRng removed in rand 0.10 → SysRng/TryRng)
- https://docs.rs/aead/0.6.1/aead/trait.Aead.html (encrypt/decrypt with Payload{msg,aad})
- https://docs.rs/zbus/latest/zbus/struct.MessageStream.html (zbus 6.4: for_match_rule)
- https://crates.io/api/v1/crates/rfd (0.18.0, xdg-portal/wayland features by default, gtk3 optional)
- https://v2.tauri.app/security/capabilities/ (application commands allowed by default)
- https://registry.npmjs.org/vite/latest (8.3.8) ; …/@sveltejs/vite-plugin-svelte/latest (7.3.1) ; …/svelte-check/latest (4.7.8) ; …/vitest/latest (5.1.1) ; …/@tauri-apps/api/latest (2.12.5) ; …/@tauri-apps/cli/latest (2.12.6)

## 11. Internationalization

- **Catalogs**: `ui/src/locales/<code>.json`, flat keys (`section.name`), values with `{param}`
  and plurals `<key>_one` / `<key>_other`. `fr` is the reference (all keys), then `en` as fallback.
  Shipped languages: fr, en, it, de, es, pt. `_name` = the language name in its own language.
- **Web** (`ui/src/lib/i18n.svelte.ts`): `import.meta.glob` discovers the catalogs; `t`, `tn`
  (Intl.PluralRules), `formatDateTime` (Intl), `setLocale` (memorized in localStorage for the unlock
  screen), initial language = memorized → system language → fr. Reactive (`$state`).
- **Native** (`src-tauri/src/i18n.rs`): the same JSON embedded via `include_str!` (`native.*` keys),
  `tr` / `tr_with`. The language is read from settings when a VTE window opens and carried by
  `VteWindow.locale` (menus, dialogs, status bar, texts written into the terminal).
- **Backend errors**: Rust returns French strings; `ui/src/lib/errors.ts` maps them (regex +
  parameters, recursive wrappers) to `be.*` keys — plugged into `message()` (api.ts).
- **Guardrails** (vitest + cargo test): key and `{param}` parity across languages; no hardcoded
  visible text in `.svelte` files (AST parse); every `locales/` file declared in `LOCALES`.