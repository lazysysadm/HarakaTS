# USER GUIDE — HarakaTS

> User and developer guide. Updated at every milestone.
> **Current state: M1 shipped (vault + crypto + export/import + minimal UI)** —
> Phase 0 docs validated on 2026-11-20. Build and tests via the Podman container (§4);
> the "Usage" sections activate as milestones M2 → M7 land (see ROADMAP.en.md).

## 1. What HarakaTS is

An SSH/SFTP connection manager for Linux (tabbed terminals, SFTP panel, remote editing,
tunnels, multi-exec), with a **portable** encrypted vault: an export re-imported on another
machine recovers every password and every key, without re-entry. The OS keyring (KWallet)
is only an optional convenience: **without it, everything works**, with the master
password.

## 2. Dev environment architecture (hybrid Podman + host)

Validated choice: **compile and test inside a Podman container** (the workstation OS stays
clean), **run the GUI app on the host** with the produced binary.

```
┌─────────────────────────────┐        ┌──────────────────────────────┐
│ Podman container (dev)      │        │ Host (KDE)                   │
│  Rust ≥ 1.90, Node 22, pnpm │ build  │  runtime libs webkit2gtk-4.1 │
│  webkit2gtk-4.1-dev, GTK3   │ ─────▶ │  ./target/release/harakats   │
│  openssh-server (tests)     │        │  data ~/.local/share/…       │
│  cargo test / clippy / audit│        │  (GUI, optional keyring)     │
└─────────────────────────────┘        └──────────────────────────────┘
```

- The source code is mounted into the container (`/work`); you edit with your usual tools
  on the host.
- The container is built from `packaging/Containerfile` (created at M1, content below);
  image `local/harakats-dev`.
- SSH/SFTP tests run **inside** the container (local sshd port 2222 + in-process russh
  server): no nested container, no service on the host.
- Acknowledged limit: no `tauri dev` hot-reload on the host without a host toolchain; the
  dev loop is "edit → container rebuild → relaunch the binary". If you ever want
  hot-reload, we will look into installing rustup/fnm on the host (your choice).

## 3. Prerequisites to install YOURSELF (I have no root, I install nothing)

The dev workstation is **Fedora 45 KDE** (SELinux enforcing: the project's Podman mounts
must carry `:z`). On the host, only the **runtime** libraries to run the produced binary
(Podman is already in place):

```bash
sudo dnf install -y webkit2gtk4.1 gtk3 libsoup3 javascriptcoregtk4.1 glib2
podman --version
```

On a Debian 12+ / Ubuntu 22.04+ target (another machine), the apt equivalent:

```bash
sudo apt-get update
sudo apt-get install -y libwebkit2gtk-4.1-0 libgtk-3-0 libsoup-3.0-0 libjavascriptcoregtk-4.1-0 libglib2.0-0
```

Notes:
- The produced packages declare their runtime dependencies:
  `apt install ./harakats_*.deb` (Debian/Ubuntu) or `dnf install ./harakats-*.rpm`
  (Fedora; RHEL/Rocky/Alma **10** with EPEL enabled) pull everything automatically.
- **RPM targets**: Fedora ≥ 42, EL10 via EPEL, Mageia 9/10. **EL9 (RHEL/Rocky/Alma 9)
  not supported**: webkit2gtk4.1 is absent there and glib2 is too old for Tauri v2's soup3
  stack.
- **KWallet (KDE)**: optional. To enable the unlock cache, KWallet must expose the Secret
  Service interface (KDE wallet settings). Without it, HarakaTS starts and works normally,
  master password at every unlock — no blocking message.
- No package is installed on the host for development: everything is in the container.

## 4. Dev container (from M1)

> **Step-by-step operational tutorial: [`build_now.en.md`](build_now.en.md)** (prerequisites,
> image, full/quick build, reinstall, troubleshooting — single copyable blocks).

`packaging/Containerfile` (planned content, created at M1):

```dockerfile
FROM docker.io/library/debian:bookworm
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential curl wget file pkg-config ca-certificates git \
      libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
      libjavascriptcoregtk-4.1-dev librsvg2-dev libssl-dev libdbus-1-dev \
      openssh-server openssh-client xz-utils unzip rpm \
    && rm -rf /var/lib/apt/lists/*
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain stable --profile minimal
ENV PATH="/root/.cargo/bin:${PATH}"
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y nodejs && corepack enable
WORKDIR /work
```

Debian bookworm (glibc 2.36) is chosen so the produced binary runs on Debian 12+ /
Ubuntu 22.04+ (glibc forward compatibility).

Building and entering the container (single block, run from the project root):

```bash
podman build -t local/harakats-dev -f packaging/Containerfile packaging/
podman run --rm -it --name harakats-dev \
  -v /opt/PROJECTS_AI/HarakaTS:/work:z \
  -v harakats-cargo-registry:/root/.cargo/registry \
  -v harakats-cargo-git:/root/.cargo/git \
  -v harakats-pnpm-store:/root/.local/share/pnpm/store \
  -w /work local/harakats-dev bash
```

Inside the container, M1 development loop (single block — the UI is built **before**
clippy/tests because `generate_context!` embeds `ui/dist` at compile time):

```bash
cargo run -p xtask -- all
bash crates/haraka-ssh/tests/sshd/setup.sh
export HARAKA_SSHD_TEST=1 HARAKA_TEST_KEY=/work/crates/haraka-ssh/tests/sshd/run/client_ed25519 HARAKA_TEST_USER=harakats-test HARAKA_TEST_PORT=2222
pnpm install
pnpm --dir ui install
pnpm --dir ui build
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm --dir ui lint
pnpm --dir ui check
pnpm --dir ui test
pnpm tauri build --bundles deb,rpm
```

`xtask all` generates the PNG icons (required by the bundler) and the versioned v1 format
fixtures (`tests/fixtures/*.hrk*`, public test passphrases). The Tauri CLI is installed at
the root (`pnpm tauri`, not `cargo tauri`). First build: crate downloads and compilation
(~10–20 min); subsequent ones are incremental.

On the host, launch the produced app (single block):

```bash
/opt/PROJECTS_AI/HarakaTS/target/release/harakats
```

(Iterative dev: `cargo build` then `target/debug/harakats. Produced packages:
`target/release/bundle/deb/*.deb`, `target/release/bundle/rpm/*.rpm`.)

### 4.1 Useful environment variables (diagnostics)

| Variable | Effect |
|---|---|
| `RUST_LOG=harakats=debug,info` | detailed backend logs (session open/close, resize, output batches, errors). Capture: `RUST_LOG=harakats=debug,info harakats 2>/tmp/harakats.log` |
| `GDK_BACKEND=x11` | forces X11 instead of Wayland (isolate a Wayland issue) |
| `WEBKIT_DISABLE_COMPOSITING_MODE=1` | disables Tauri webview compositing (isolate a GPU/webview issue; no effect on the native VTE terminal) |

## 5. Using the application

### 5.1 First start (M1 — available)

1. Launch HarakaTS → vault creation screen.
2. Choose a strong **master password** (long passphrase recommended).
   It encrypts everything; **forgotten = data lost** (no backdoor).
3. Argon2id parameters are calibrated automatically (~0.5–1 s on your machine).
4. Optional: enable the KWallet cache (Settings → vault). Without KWallet, nothing changes
   except typing the master password at every unlock.

### 5.2 Connections and credentials (M2 — available)

- Folder tree (drag & drop with cycle protection, colors, tags, icons, inline rename,
  keyboard navigation ↑/↓/Enter); a folder can define default values (user, port,
  credential, jump host, clipboard policy, keepalive, encoding) inherited by its children.
- The connection editor shows the **effective configuration with provenance** ("system
  default", "inherited from <folder>", "connection override"); each field has an "inherit"
  checkbox to restore the default value.
- **Credentials are decoupled**: create a credential (login + password and/or key +
  passphrase), reference it from N connections; one edit suffices everywhere. Deletion:
  refused if referenced, or explicit detachment of the references.
- **Reveal / Copy** a secret: explicit audited actions; the copy goes through the backend
  and is cleared from the clipboard after N seconds (adjustable); the on-screen reveal
  auto-hides after 30 s.
- **Import a private key from a file** (Credentials → "Import a file…", initial folder
  `~/.ssh`): backend-side reading only — the key goes through neither the clipboard nor
  the interface; only metadata is displayed (type, SHA256 fingerprint, encrypted or not).
  Encrypted key: the passphrase is **verified before saving**. Public key (.pub) refused
  with an explanation; warning if the source file permissions are too broad; the sibling
  public key `<file>.pub` is displayed if present. Audited import (file name + fingerprint,
  never the content).
- **Ctrl+K**: fuzzy search (connections, folders, credentials, actions); favorites in the
  sidebar. The quick connect bar was removed (it served no purpose without existing
  credentials); a connection is created from the tree (group button or right click).
  The toolbar contains only **☰ Menu** (SFTP, editor, tunnels, multi-exec, macros,
  credentials, import/export, audit, settings, palette, About) and **🔒**.
- **FR/EN** interface (Settings → language). Per-folder export available (M2).

### 5.3 SSH terminal (M3 / M3-VTE — available)

- **Default engine: native VTE (GTK) — Remmina's engine.** Every connection (▶,
  double-click, Ctrl+K) opens a **native terminal window** 1100×700: rendering,
  keyboard, resize and copy/paste (native Ctrl+Shift+C/V) handled by the system, outside
  the webview — htop/tmux/vim can no longer freeze the display. The server PTY is opened at
  the window's real dimensions; resize is reflected on the fly (100 ms debounce). Auth and
  host-key prompts = native dialogs. Closing the window closes the session; locking the
  vault closes everything.
- **Native status bar** below the terminal: state/latency, cols×rows dimensions,
  **Reconnect** button. Connection errors are displayed in red INSIDE the terminal.
  **Context menu** (right click): Copy, Paste, Select All, Clear Screen, Reset Terminal.
- **Windowed mode (default)**: a session bar below the toolbar (one pill per session,
  green/yellow/red state dot; a click brings the terminal back, ✕ closes the session) and
  the SFTP panel opening on the active session.
- **Tabbed terminals** — Settings → "Terminals in tabs inside the main window"
  (applies to sessions opened afterwards): an mRemoteNG-style layout. On
  the left, the **connections column** (tree, menu): narrow (240 px by default) and
  **resizable with the mouse between 200 and 420 px**; on the right, the **tabbed
  terminals** (colored state dot, ✕ to close; at least 360 px). When a wide view opens
  (SFTP, editor, tunnels, multi-exec, settings windows…), the column automatically switches
  to a **fixed** width (~55%, 880 px minimum), then returns to the chosen width on close.
  Switching tabs changes the active session.
- **SFTP on demand**: the **Files (SFTP)** button in the terminal's status bar (next to
  Reconnect) opens the SFTP panel on that terminal's session. "Follow terminal" remembers
  the last folder of each shell (OSC 7): it survives tab changes. In tabs mode, nothing
  opens by itself.
- **Server status** (option, off by default) — Settings → "Show server status (green /
  red dot)": a green/red/grey dot in front of each connection in the tree. The probe is a
  plain **TCP connection** to the SSH port, closed right away (no ICMP, no SSH exchange);
  adjustable interval (60 s by default, 15 s minimum), one test per host:port, at most 8 in
  parallel, 2 s timeout, nothing if the vault is locked; connections behind an SSH jump
  host are not tested (grey).
- **Output colorization** — Settings → "Colorize terminal output (errors, warnings, IPs,
  URLs)" (on by default): fixed rules, red (error, failed, fatal, critical, denied,
  refused…), yellow (warning, deprecated, timeout), green (success, ok, passed, enabled,
  `active (running)`), cyan (IPv4, URL).
  Dates, times and paths are not colorized. Never applied inside a fullscreen program
  (vim, htop, less, tmux), nor over a color set by the server (`ls --color`,
  `grep --color`). Session logs stay raw. Applies to sessions opened afterwards.
- Input and resize are processed **in order** by a single task per session; coalesced
  output (a single GTK send in flight); font and scrollback configurable in Settings
  (applied when windows open).
- **Single engine: native VTE** (no more engine choice in Settings; the xterm.js web
  fallback, its tabs, splits and detachments were removed in M6).
- Auth: password, key (ed25519/ecdsa/rsa, encrypted keys accepted), 2FA/OTP
  (keyboard-interactive, on-screen prompts), local SSH agent — in the order defined by
  inheritance; material is resolved by the backend.
- First connection to a host: SHA256 fingerprint displayed, accept/reject (TOFU). If the
  key changes later: **blocking alert** — do not accept blindly. Settings → "Host keys
  (internal known_hosts)": fingerprint list + `~/.ssh/known_hosts` import.
- Jump hosts: configurable bastion chain per connection (inheritable).
- Keepalive + displayed latency, **automatic reconnection** (backoff), configurable
  terminal encoding. Web engine: search, clickable links, Unicode 11, Ctrl+Shift+C/V via
  the backend clipboard.
- On vault lock: all sessions are closed.

### 5.4 Clipboard (M4 — available)

- **Inheritable policy** per group/connection: `disabled` / `l2r` (local→remote) /
  `r2l` (remote→local) / `bidirectional` (default). Settings → default policy; the
  connection editor shows the effective value + provenance.
- **OSC 52 intercepted in the backend** (sequences never reach the VTE terminal):
  remote→local write subject to `r2l` + the `osc52_max_bytes` limit (default 100 KB);
  **remote reading of the local clipboard (query) subject to `l2r` and always audited**
  (exfiltration vector — set the policy to `r2l` or `disabled` on untrusted hosts).
- **Paste confirmation** (`paste_confirm_multiline` setting, on by default): any multiline
  paste or one containing control characters/escapes opens a dialog with an **exact
  preview** (␛ ␍ ␊ rendered visibly) — anti-pastejacking. Native GTK dialog. Cancel sends
  nothing.
- Optional **copy-on-select** (setting): the selection goes to the clipboard; native Linux
  primary selection (middle click). Ctrl+Shift+C/V in both engines.
- "Copy" on a secret clears it from the clipboard after 30 s (adjustable). Warning:
  Klipper may keep a history — this is inherent to the OS.
- File copy-paste between panels and OS→app drag & drop: shipped with the SFTP panel
  (§5.5).

### 5.5 SFTP and transfers (M5 — available)

**Open**: the **📁 Files (SFTP)** button in the top bar (toggle), or the **Files (SFTP)**
button in the terminal window's status bar (opens the panel on THAT session). **It also
opens by itself** when a session opens and follows the **active session** (focused terminal
or pill chosen in the session bar, §5.3). The panel is an **integrated, non-modal view**: it
occupies the central area of the main window; the connection tree and the toolbar remain
usable to launch other sessions while transfers run. "Close the panel" (or clicking the
button again) hides it without losing state (folders, selection, transfers). The panel
reuses the terminal's SSH connection (second channel, `sftp` subsystem): **no second
authentication**. "Session" selector: connected sessions; changing it also brings the
corresponding terminal to the foreground.

- **Two panels**: Local (left) / Remote (right) — clickable breadcrumb, ↑ parent, ⟳,
  hidden files (checkbox), filter, sort by name/size/date (header click), multiple
  selection (Ctrl+click, Shift+click), double-click = enter a folder (or transfer a file).
  Symbolic links to a folder are traversed.
- **Transfer**: "Send ➜" / "➜ Download" buttons (the destination is the displayed folder
  of the other panel), **drag & drop from one panel to the other**, or **from the file
  manager into the window** (upload to the displayed remote folder). "Files…" / "Folder…"
  open the native picker.
- **Options**: if the destination exists → skip (default) or replace; symbolic links →
  skip (default) or follow; preserve rights and dates (default yes).
- **Remote operations**: new folder, rename, rights (octal, optionally recursive),
  symbolic link, properties, copy path, delete (folder = recursive, confirmed).
- **Transfers**: queue with progress, throughput, ETA, completed files; parallelism in
  Settings → "Parallel transfers" (default 3, max 8); ⏸ pause, ▶ resume, ✕ cancel.
  Folders are expanded recursively (folders before content).
- **Resume after interruption**: data lands in `<file>.part`, renamed only at the end
  (never a half-written file under its real name). Pause, error or network drop → ▶ (or
  relaunch the same transfer) restarts from the `.part` offset, after comparing the last
  64 KiB on both sides: if the source changed or the `.part` is corrupted, the transfer
  **restarts from zero**. Cancel deletes the `.part` of the file in progress. Integrity:
  the **size** is verified at the end (SFTP v3 has no standard hash).
- **Path bar**: above each panel, an editable field — type a path then Enter (`/var/log`,
  `../tmp`, `~/docs`, `~` = home folder; relative to the displayed folder); Escape restores
  the current path. The clickable breadcrumb remains available.
- **Follow the terminal (OSC 7), MobaXterm-style**: the **Follow terminal** checkbox (top
  of the panel). **Checking it** enables following: if the session shell has not yet
  announced its folder, HarakaTS **types a line** into the shell (bash or zsh) that
  announces it at every prompt; the remote panel then follows your `cd`s. No remote file is
  modified, the effect ends with the shell. Do it **at the prompt** (not in vim/htop/less:
  the line would be typed there). **The line is hidden**: the echo is rendered invisible
  (SGR "conceal" attribute), then erased as soon as the shell answers; only a normal prompt
  remains (fallback: if the screen scrolled or the prompt spans several lines, an extra
  empty prompt may remain). Idempotent. **Automatic**: Settings → "SFTP: automatically
  enable terminal folder following (injects a line into the shell)" injects this line on
  every connection/reconnection of a native session (off by default). For permanent
  following without injection: *Transfer options → Copy OSC 7 snippet*, to paste into the
  server's `~/.bashrc` / `~/.zshrc`. Fedora/Ubuntu with `vte.sh` sometimes already emit OSC 7.
- **Layout**: the view is aligned with the connection tree (same top and bottom) and fills
  the available height without scrolling the whole view: compact top bar (session, ⟳,
  follow, collapsed *Transfer options*), two panels whose action buttons sit in each
  panel's header, transfer list at the bottom (scrollable).
- **Acknowledged v1 limits**: drag-out (panel → file manager) not supported by Tauri
  webviews (use "Download"); non-pipelined transfers (one 64 KiB block per round trip:
  throughput bounded by latency on slow links); folder permissions not preserved (files:
  yes); `chown` not exposed; `check-file` extension not used; the local panel is read-only
  (+ "New folder").
- Audit log: `sftp_transfer` (direction), `sftp_remove` (never paths or contents).

### 5.6 Remote editing (M6 — available)

**Open**: in the SFTP panel, **double-click a remote file** (≤ 5 MiB, otherwise download),
or select ONE file then **Edit** (integrated editor) / **Edit externally**. The
**📝 Editor (n)** button in the top bar shows the Editor view (non-modal, same layout as
SFTP; several documents in tabs, the connection tree remains usable).

- **Integrated editor (CodeMirror 6)**: highlighting based on the file extension (modules
  lazy-loaded), line numbers, search/replace (Ctrl+F), history (Ctrl+Z), indentation (Tab),
  matching brackets, theme following the application's. **Ctrl+S** saves: the file is sent
  back to the server immediately (rights preserved).
- **Line endings**: LF / CRLF preserved byte for byte; selector in the status bar to
  convert on save. Non-UTF-8 file or > 20 MiB: integrated editing unavailable (external
  editor possible for non-UTF-8).
- **Conflict**: if the remote file changed since opening (date/size then SHA-256 content;
  a mere `touch` is not a conflict), saving is refused and a bar offers: **Overwrite**
  (impose your version), **Reload** (discard your changes, restart from the remote) or
  **Compare** (side-by-side remote/local view, editable, then "Save merged result"). File
  deleted in the meantime: "no longer exists" → Overwrite recreates it.
- **External editor**: Settings → **External editor** (command line; `{file}` = temporary
  file, otherwise the path is appended; **no shell** is used; empty = system default
  application via `xdg-open`). Every save in the editor is sent automatically (after ~0.4 s
  of quiet); on conflict, the same bar appears in the Editor view.
- **Atomic send**: write into a temporary file in the same folder, rights copied, then
  replacement (SFTP does not allow overwriting by rename: the old file is set aside then
  deleted, and restored on failure). No partial file is visible under its name; for a few
  milliseconds the file may be momentarily absent for a concurrent reader.
- **Temporary copies**: `<data>/tmp/<uuid>/` (directory 0700, file 0600), deleted on
  document close, on vault lock and purged at startup (crash). External editors may create
  their own copies (swap, backups): caution with sensitive files.
- Audit: `edit_open`, `edit_save`, `edit_conflict`, `edit_external` (never a path or
  content).

### 5.7 Tunnels, multi-exec, macros, logging, imports (M7 — available)

**SSH tunnels (L / R / D)** — the **🔀 Tunnels (n)** button (non-modal view, like SFTP and
the editor).
- *Definition*: in the connection editor, "Tunnels" section: type **local** (`-L`, a local
  port reaches a destination through the server), **remote** (`-R`, a server port reaches a
  destination on your machine), **dynamic** (`-D`, local **SOCKS5** proxy). Bind address
  **127.0.0.1 by default**; a non-loopback address exposes the port on the network (warning
  displayed, never blocked). "Start automatically" checkbox: launched on every session
  (re)connection.
- *Live*: the view lists the tunnels of all sessions (state, real port — port 0 is assigned
  by the OS/the server —, active connections, bytes), **hot Start / Stop**, ad-hoc tunnel
  addition on a connected session. Closing the session or locking the vault stops its
  tunnels.
- *SOCKS5*: CONNECT only (IPv4, IPv6, domain name), without authentication; BIND and UDP
  refused. Usage: `curl --socks5-hostname 127.0.0.1:<port> https://…`.
- *Remote*: depends on server-side `GatewayPorts` for a non-loopback bind (advice displayed).
- *Limits*: 256 simultaneous connections per tunnel, 10–15 s timeouts (channel open, SOCKS
  handshake); a tunnel from before a reconnection is cut, only auto-start ones come back;
  `bind_addr` must be an IP, `localhost` or empty.
- Audit: `tunnel_start` / `tunnel_stop` (type and port, never the destination).

**Multi-exec** — the **Multi-exec** button: check ≥ 2 connected sessions, **confirm**, the
input (keystrokes, paste, macros) of one member window is sent **byte for byte and in
order** to the others. Permanent indicator: "MULTI-EXEC (n)" label in each member window's
status bar and a red banner in the main window. The panel is **floating and non-modal**
(movable by its header, collapsible, resizable): the interface remains usable while it is
open. Once active: "Send to all sessions" bar (line + Enter) and **sending an existing
macro** (selector; exact preview and confirmation if the macro requests it; the macro goes
to one member and is broadcast to the others, audited like any macro send). Disables itself
when < 2 members remain (window closed, session closed, lock). Copies never go back through
the other sessions' input queue (no loop).

**Macros / snippets** — the **Macros** button: list, creation/editing (name, multiline
content, "Confirm before sending"). The content is **encrypted in the vault** and hidden by
default (Show content = explicit audited action). **Send** to a session from the Macros
window or via the **terminal right-click menu** ("Macros" submenu); a macro marked
"confirm" first shows the exact preview (visible controls). Each line break becomes Enter
(the last line gets Enter only if the content ends with a line break). **Startup snippet**
(connection editor): sent ~0.7 s after connection, once per session; a "confirm" macro is
**ignored** at startup (note in the terminal + audit) so it never blocks on a dialog.

**Session logging** (opt-in per connection, checkbox in the editor): terminal output
written to `<data>/logs/<name>-<YYYYMMDD-HHMMSS>.log` (**0600** file, 0700 folder), escape
sequences stripped, `\r` overwrites the line (progress bars), 50 MiB cap, flush every 2 s.
⚠ A typed password **may** appear if it is echoed by the session. OSC 52 payloads never
reach the log. Audit: `session_log_start` / `session_log_stop`.

**Imports** — Export / Import → "Import from another application": **`~/.ssh/config`**,
**Remmina** files (`.remmina`, one or several, or a folder), **MobaXterm**
(`.mxtsessions`). Preview before applying (counters, connections, warnings, passphrase
keys), then report. Everything lands in an "Import <source>" group to review; nothing
existing is modified (conflicting names get a suffix). **An import never crashes**: binary,
huge, cyclic or corrupted entries → warning report.
- *ssh_config*: `Host` (`*`/`?` patterns, `!` negation), `Include` (glob, depth 8, cycle
  guards), `HostName`, `User`, `Port`, `IdentityFile` (the key is **imported into the
  vault**, one credential per file; encrypted key → passphrase to re-enter), `ProxyJump`
  (chains), `ServerAliveInterval`, `Local/Remote/DynamicForward` → tunnels. `Host *` →
  group default values. **Not supported, flagged**: `Match` (block ignored),
  `ProxyCommand`; other options are listed in a warning.
- *Remmina*: `protocol=SSH` only, `server`, `username`, `name`, `group` (subgroups),
  `ssh_privatekey`, SSH tunnel → jump. The password is encrypted by Remmina with its local
  key: **to re-enter** (decryption via `remmina.pref` is not implemented).
- *MobaXterm*: `[Bookmarks*]` sections, `SubRep` → nested groups, SSH sessions (`#109#`);
  other types ignored (a warning); passwords **to re-enter**; embedded keys not imported.
- Secrets and key contents never travel to the interface (backend-side preview).

### 5.8 Portable export / import (M1, criterion #1)

1. Settings → Export: choose the scope (everything / one folder / no secrets), an **export
   passphrase distinct from the master password**, the `.hrkx` file.
2. On the other machine: HarakaTS → Import → select the `.hrkx`, enter the export
   passphrase, choose the conflict strategy (keep / replace / duplicate).
3. Secrets are re-encrypted with the local vault's DEK: every connection opens with its
   credentials, without re-entry.
4. Carry the bundle and the passphrase over separate channels. A tampered bundle (even one
   bit) is rejected.

### 5.9 Locking

- Auto-lock after inactivity (15 min by default, adjustable), on sleep and on KDE session
  lock. On lock: vault keys erased from memory, decrypted remote files wiped and, by
  default, **all sessions closed**. Settings → "Close all sessions when the vault is
  locked" (unchecked): SSH sessions and tunnels stay open, terminals are **hidden** while
  the vault is locked and come back on unlock. Trade-off: the connections stay alive and
  the secrets already used remain in memory until they are closed.

### 5.10 Main window

The application starts in **windowed mode** (1280×800 by default, minimum 940×600), never
maximized by default. On close, the size (and maximized state) is saved in
`window_state.json` (data folder) and restored at the next launch. Deleting this file
restores the default size. The position is not memorized (Wayland leaves it to the window
manager).

### 5.10bis Appearance and accessibility

- **Design variables** (`ui/src/app.css`): radii (`--radius-*`), shadows (`--shadow-*`), text
  sizes (`--fs-*`), spacings (`--sp-*`), transition (`--t-fast`) and semantic colors per theme
  (`--warn`, `--fav`, `--alert`, `--ok-strong`, `--bad`, `--neutral`, `--term-bg`, `--overlay`). No
  more hardcoded colors outside the theme definitions: the 4 themes (System / Gray / Blue /
  Light) apply to the whole interface (state pills, multi-exec banner, favorites…).
- **Keyboard**: visible focus ring (2 px, accent color) on all interactive elements; short
  transitions (140 ms) disabled when the system requests reduced motion
  (`prefers-reduced-motion`).
- **Contrast**: the text colors of every theme meet WCAG AA (≥ 4.5:1; ≥ 3:1 for graphical
  elements). The light theme was fixed (`--muted`, `--accent`, `--ok` darkened). The
  `ui/src/lib/theme-contrast.test.ts` test checks these ratios for the 5 color sets.
- **Native** areas (GTK tabs and status bar, terminal windows) do not follow these themes
  yet.

### 5.11 About and license

The **ℹ About** button (top bar): version, platform, main components (Tauri, russh,
russh-sftp, GTK+VTE, Argon2id / XChaCha20-Poly1305), data folder and the **full text of the
project's MIT license** (`LICENSE` file at the root, embedded at compile time and declared
in the Cargo metadata and in the .deb/.rpm packages). There is no longer a version banner
at the bottom of the window; error messages appear in a **notification** at the bottom
right (closable with ✕).

**Third-party licenses**: the "Third-party licenses" button in About → notices and full
texts of the integrated Rust and npm components (`THIRD-PARTY-LICENSES.md`, generated by
`cargo run -p xtask -- licenses` via cargo about + pnpm licenses, embedded at compile time
and bundled in the packages). **CI guardrail**: `cargo deny check licenses` (`deny.toml`) —
only MIT-compatible permissive licenses are accepted (MIT, Apache-2.0 ± LLVM-exception,
ISC, BSD, CC0, 0BSD, MIT-0, Unicode, Zlib, BSL, file-level MPL-2.0); any GPL/AGPL/unknown
fails the CI. The **LGPL** system libraries (GTK3, glib, VTE, WebKitGTK) are linked
**dynamically** via the distro packages — never statically (LGPL relocatability
obligation). Audit of 2026-11-21: no conflict with the project's MIT license.

### 5.12 Languages (i18n)

**Shipped languages**: Français (reference), English, Italiano, Deutsch, Español,
Português. Settings → **Language** (list generated automatically). Before vault unlock
(password screen), the language is the one memorized at the last choice, otherwise the
system's if supported, otherwise French; after unlock, it is the vault setting.

**All text is externalized**: one JSON file per language in `ui/src/locales/<code>.json`
(flat keys `section.name`, `{name}` parameters, plurals `key_one` / `key_other`). The same
file feeds the web interface (Svelte) AND the native GTK/VTE interface (`native.*` keys,
embedded at compile time by `src-tauri/src/i18n.rs`). Backend error messages (in French in
the Rust code) are translated on the interface side by `ui/src/lib/errors.ts` (one rule
per message, extracted parameters); an unlisted message stays displayed as-is.

**Add a language** (e.g. Dutch `nl`):
1. copy `ui/src/locales/fr.json` to `nl.json`, translate the values (never the keys or the
   `{parameters}`), set `_name` to "Nederlands";
2. add a line `("nl", include_str!("../../ui/src/locales/nl.json"))` in `LOCALES`
   (`src-tauri/src/i18n.rs`) — the Rust test `all_locale_files_are_registered` fails
   otherwise;
3. run `pnpm --dir ui test`: `i18n.test.ts` checks that all keys exist, that no value is
   empty and that `{parameters}` are preserved. The language appears by itself in Settings.
**Add a text**: create the key in `fr.json` (then in the other languages; otherwise
fallback English → French), use it via `t("key", {param})` / `tn("key", n)`. The
`i18n-guard.test.ts` test fails if a Svelte component contains hardcoded visible text.
**Limits**: native terminal windows keep the language in effect when they opened; texts
returned by the remote server (banners, authentication prompts) are not translated; Rust
error messages absent from `errors.ts` stay in French.

### 5.12bis Versions and publishing

- **SemVer** scheme: `0.N.0` at each functional milestone, `0.N.P` for a fix, `1.0.0` when
  the §5.13 acceptance checklist is fully validated.
- Change version: `cargo run -p xtask -- version set 0.9.0` (modifies `Cargo.toml` then
  aligns the JSONs), `cargo build` (updates `Cargo.lock`), commit, then
  `git tag -a v0.9.0 -m "…"` and `git push origin main --tags`. `version check` (CI)
  refuses any misalignment.
- The version is visible in "About" with the build's git hash, and via `harakats --version`.
- The vault/export format has its own version (v1, frozen): independent of the
  application's.

### 5.12ter Git branches

| Branch | Role |
|---|---|
| `main` | **Stable**: only validated code, every published version is tagged `vX.Y.Z` there. |
| `dev` | **Integration**: finished features are merged there before being validated together. |
| `feat/<name>` | **One feature or a large work package** (e.g. `feat/sftp-only`), created from `dev`. |
| `fix/<name>` | Targeted fix; an urgent fix starts from `main` and is also merged into `dev`. |

Flow: `feat/x` → `dev` (tests in the container) → `main` (version + tag + packages). Small
fixes and documentation: directly on `dev` or `main`. Rollback: `git checkout vX.Y.Z` or
`git revert`. The `stable-*` tags mark exceptional rollback points.

### 5.13 v1 acceptance checklist (clean machine)

To run on a virgin Linux machine (Fedora ≥ 42 / EL10+EPEL / Debian 12+ / Ubuntu 22.04+),
KDE or without a keyring. Check every line; any unchecked box = an anomaly to report with
`/tmp/harakats.log` (`RUST_LOG=harakats=debug,info harakats 2>/tmp/harakats.log`).

**Installation**
- [ ] `rpm -qp --requires <package>.rpm` (or `dpkg-deb -I <package>.deb`) lists `vte291` / `libvte-2.91-0` and WebKitGTK 4.1;
  the package license is MIT.
- [ ] Error-free installation (`sudo dnf install ./haraka-ts-*.rpm` or `sudo apt install ./haraka-ts_*.deb`);
  the "HarakaTS" entry appears in the application menu with its icon.
- [ ] First launch: vault creation (master password), no mandatory keyring request
  (without Secret Service/KWallet: informational message only).
- [ ] Portable mode: a `portable.marker` file next to the binary (or `--data-dir <folder>`) places the data in that folder.

**Vault and portability (criterion #1)**
- [ ] Create a group, a connection and a credential (password + key), **Export / Import → export everything** with a passphrase.
- [ ] On another machine (or `--data-dir /tmp/profileB`): create a virgin vault, **import** the bundle: all secrets
  are recovered **without re-entry** (reveal/copy the password; open a session with the key).
- [ ] Lock then unlock; auto-lock after inactivity; locking closes sessions and empties `tmp/`.

**Sessions, terminal, SFTP, editing**
- [ ] Open a session (password, then key, then 2FA/OTP if available); host-key TOFU then change alert.
- [ ] `htop`/`vim`: enlarge/shrink the window, quit cleanly; copy/paste (Ctrl+Shift+C/V, middle click, confirmed multiline paste).
- [ ] Jump host (connection with a bastion).
- [ ] SFTP: browse, drag & drop a large file, **pause then resume**, cut the network then resume, recursive folder.
- [ ] Edit a remote file (integrated editor), save; trigger a conflict and test Overwrite / Reload / Compare; external editor.

**Sessions, tabs, colorization (0.8.0)**
- [ ] 2 open connections: the session bar shows 2 pills; clicking brings the terminal back; the SFTP panel opens by itself and follows the active terminal; ✕ closes the session.
- [ ] "Terminals in tabs" option checked, new sessions: tree on the left (240 px, resizable 200–420 px), tabs on the right, tab ✕ closes the session; "Files (SFTP)" opens the panel (wide fixed column) and closing it gives the space back to the terminal.
- [ ] Locking: by default sessions close; box unchecked, terminals hidden then restored on unlock.
- [ ] Server status enabled: green/red dots in the tree; ⓘ tooltips in Settings.
- [ ] Colorization: `journalctl -n 50` / `systemctl status sshd` colorize error/warning/`active (running)`, IPs and URLs; `ls --color`, `vim`, `htop` unchanged; unchecked box = no color.

**Tunnels, multi-exec, macros, imports**
- [ ] Local (`-L`), remote (`-R`) and dynamic SOCKS5 tunnel (`curl --socks5-hostname 127.0.0.1:<port> …`); stopping frees the port.
- [ ] Multi-exec on 2 sessions: same input in both, visible indicator; floating panel (interface usable behind); existing macro sent from the panel; startup snippet.
- [ ] Session logging enabled: 0600 log file in `<data>/logs/`.
- [ ] Imports: `~/.ssh/config`, a `.remmina`, a `.mxtsessions` → crash-free report, connections in "Import …".

**Languages and interface**
- [ ] Change the language (6 available): no residual French text in the other languages.
- [ ] The window starts windowed and restores its size at the next launch.

## 6. Data location

| Item | Path (default) |
|---|---|
| Vault | `~/.local/share/harakats/vault.hrk` (0600) |
| Rotating backups (5) | `~/.local/share/harakats/backups/` |
| Internal known_hosts | `~/.local/share/harakats/known_hosts` |
| Audit log (no secrets) | `~/.local/share/harakats/audit.log` |
| Session logs (opt-in) | `~/.local/share/harakats/logs/` |
| Editing temporaries | `~/.local/share/harakats/tmp/` (purged) |

**Portable mode** (M7): `portable.marker` next to the binary or `--data-dir <path>` →
data in `./data/` (USB stick, synchronized repository). Since the vault is encrypted, cloud
synchronization is safe **if the passphrase is strong**.

## 7. Troubleshooting

| Symptom | Cause / solution |
|---|---|
| `error while loading shared libraries: libwebkit2gtk-4.1.so.0` | missing runtime libs → apt block §3 |
| `dnf install`: "nothing provides webkit2gtk4.1" | EL9 not supported (see §3); on EL10, enable EPEL first (`dnf install epel-release`) |
| `cargo tauri`: command not found | the CLI is installed via pnpm at the root: use `pnpm tauri build` |
| "Permission denied" in the container on `/work` | SELinux (Fedora host): the bind mount must carry `:z` (`-v …:/work:z`) |
| Corepack asks confirmation to download pnpm | variable `COREPACK_ENABLE_DOWNLOAD_PROMPT=0` (already passed in the M1 verification block) |
| "Secret Service unavailable" at startup | Normal if KWallet does not expose Secret Service: the app continues, master password required. Cache disableable in settings |
| The app does not start under Wayland | Try `GDK_BACKEND=x11 ./harakats`; native VTE terminal (single engine) |
| Native terminal window absent / VTE error | Missing `vte291` runtime → `sudo dnf install vte291` (Fedora) or `libvte-2.91-0` (Debian/Ubuntu) |
| Container binary incompatible with host (`GLIBC_2.xx not found`) | host older than bookworm → rebuild in a container aligned with the host (tell me) |
| Corrupted vault (power cut during write) | at startup, restore proposal from the last backup (`backups/`) |
| Forgotten master password | No recovery possible (security contract). Restore a `.hrkx` export if available |
| Resumed transfer fails | some SFTP servers limit offsets → the app restarts from 0 with a warning |

## 8. Security advice (user)

- Master password = long, unique passphrase; it is what protects everything.
- **Different** export passphrase, transmitted over a channel separate from the bundle.
- Workstation: encrypted swap recommended, core dumps disabled (`ulimit -c 0`) — the
  unlocked vault lives in memory (see SECURITY.en.md §8).
- Enable session logging only if necessary: terminal output may contain echoed secrets.
- Always check host key fingerprints (TOFU); a key-change alert is never trivial.
- The KWallet cache is a convenience: on a shared machine, leave it disabled.

## 9. Guide version history

| Date | Milestone | Changes |
|---|---|---|
| 2026-11-20 | Phase 0 | Creation: hybrid Podman+host environment, prerequisites, guide structure |
| 2026-11-20 | Phase 0 | Packaging: **.deb + .rpm** (AppImage removed, user decision); RPM targets Fedora ≥ 42 / EL10+EPEL / Mageia, EL9 excluded |
| 2026-11-20 | M1 | Docs validated by the user; M1 implemented: vault/crypto/export-import, normative tests, minimal UI (create/unlock/lock, settings, export/import, audit), xtask (icons+fixtures), CI. Build/tests to run in the container (§4) |
| 2026-11-20 | M1 | **M1 verified green** (run 9, authorized agent): clean clippy `-D warnings`, **35 Rust tests** (14 unit + 21 normative incl. A→B export, tampering, AAD), svelte-check **0 errors/0 warnings**, vitest **4/4**, packages `HarakaTS_0.1.0_amd64.deb` (3.73 MiB) + `HarakaTS-0.1.0-1.x86_64.rpm` (3.74 MiB); RPM deps verified via `rpm -qp --requires` (webkit2gtk4.1, javascriptcoregtk4.1, gtk3, libsoup3, glib2) |
| 2026-11-20 | M2 | **M2 verified green**: inheritance with provenance, validated tree/credential CRUD (anti-cycle, cascade, detachment), audited reveal/copy + clipboard TTL (arboard 3.7), quick connect + IPv6, Ctrl+K palette (fuzzysort 4.0.2), FR/EN i18n, KDF recalibration, schema migration; **46 Rust tests** (25 unit + 21 normative), vitest 6/6, svelte-check 0 errors, regenerated packages |
| 2026-11-20 | M2+ | Tester user feedback (app installed/launched on Fedora): delete/lock buttons **gray background**, "Dossiers" renamed **"Groupes"** (FR/EN UI), **connection search bar** (fuzzy, name/host/user), **global themes System/Gray/Blue/Light** (settings, persisted in the vault); verified green (46 Rust tests, 6 vitest, svelte-check 0/0), regenerated packages |
| 2026-11-20 | M3 | **M3 verified green** (8 runs): complete SSH terminal — password/key/encrypted-key/OTP/agent auth, TOFU host keys + blocking change + system import, chained jump hosts, keepalive/latency/reconnection, encoding, tabs/splits/detachment, Ctrl+Shift+C/V, UI prompts, known_hosts UI; **63 Rust tests** (30 core + 21 normative + 11 in-process SSH + 1 real openssh interop via container sshd), vitest 9/9, svelte-check 0/0, packages 5.89/5.90 MiB |
| 2026-11-20 | M3+ | User feedback: **Remmina-style unified interface** (top toolbar, left tree, session tabs, dialogs for editor/credentials/settings/export/audit, bottom status bar, welcome with recents), **▶ per connection + double-click = connect** + "▶ Connect" at the top of the editor + quick bar connects directly; **empty-terminal diagnosis**: visible "↓ N B" counter, backend logs (`RUST_LOG=harakats=debug,info`), robust fit (rAF + timeout) + minimum height; verified green, regenerated packages |
| 2026-11-20 | M3++ | **Empty-terminal bug FIXED**: user log `bytes=440 chars=0` → `encoding_rs::decode_to_string` does not grow the String (capacity = output limit); `String::new()` swallowed everything. Fix: capacity via `max_utf8_buffer_length`, loop over unconsumed bytes, decoder without BOM sniffing + 2 regression tests. **65 Rust tests**, 9 vitest, 0/0, regenerated packages |
| 2026-11-20 | M3+++ | **Terminal stabilization** (user feedback: shrinking at launch, misinterpreted input, disordered top/htop — cause: server PTY at a wrong size because the session opened before the final layout): session opened only at the container's real dimensions (ResizeObserver + prior fit), resize listened before opening + 100 ms debounce, post-open resynchronization, size memorized backend-side for reconnections, pre-connection input queued. Verified green (65 tests, 9 vitest, 0/0), regenerated packages |
| 2026-11-20 | M3-term2 | **Terminal anti-freeze** (user feedback: shrinking window, htop/tmux frozen and impossible to quit): ① **WebGL renderer removed** (unstable under WebKitGTK/Wayland → frozen display) in favor of the DOM renderer; ② **output batched 16 ms / 32 KB** (one IPC message per SSH packet saturated the webview); ③ stable layout: terminal absolutely positioned in a flex chain, always-visible tab bar, **default window 1280×800 maximized** (min 940×600). Research: Remmina SSH = VTE, MobaXterm = PuTTY derivative, VS Code = xterm.js (our library). Verified green (65 Rust tests, 9 vitest, 0/0), packages 5.88 MiB |
| 2026-11-20 | M3-VTE | **Terminal engine pivot (user decision)**: the log proves the backend does send htop screens (5.5 KB batches/1.5 s) but the webview rendering freezes → **native VTE (GTK), Remmina's engine, becomes the default**. Native 1100×700 window **per session**, direct libvte-2.91 FFI (LGPL; Rust bindings discarded: GPL or incompatible GTK), feed mode without local PTY (commit→SSH, size-allocate→window_change 100 ms debounce), auth/host-key prompts in native GTK dialogs (audited), window close/lock → sessions closed. "Terminal engine" setting: VTE (default) / xterm web (fallback, tabs/splits kept). Deps: RPM `vte291` (already on the host), DEB `libvte-2.91-0`, build `libvte-2.91-dev` (container + CI). Verified green (65 Rust tests, 9 vitest, 0/0), packages 5.93/5.94 MiB; 4 runs (edition 2024: `gen` reserved, `unsafe extern`; gtk-rs 0.18: `set_title(&str)`, `destroy()` unsafe, `invoke()` unit; clippy: c"" literals) |
| 2026-11-20 | M3-VTE+ | **ameliorations.md review applied (§5.1–7)**: ① resize dimensions read in idle (VTE grid up to date) + resize/connection/close logs + errors visible in red in the terminal; ② **ordered input/resize** (mpsc channel + single task per session, local encoder); ③ **russh global lock outside network awaits** (Arc<Channel>); ④ **coalesced feed** (1 GTK invoke in flight, bounded 16 MiB buffer); ⑤ tests: 50-resize burst (last size wins) + `stty size` against a real sshd + 4 keyinfo tests; ⑥ **native copy/paste + context menu** (right click), **native status bar** (state/latency/dims/**Reconnect**), prompt Cancel = empty answer; ⑦ **private key import from file** (backend only, non-secret metadata, passphrase verified before saving, .pub refused, permissions warned, audited); ⑧ toolbar **Sessions** (list + Focus + Close); ⑨ VTE font/scrollback settings; ⑩ environment variables documented (§4.1). Not applied: embedded VTE tabs (user choice of native window), ssh_config import (M7). Verified green (**71 Rust tests**, 9 vitest, 0/0), packages 5.97 MiB |
| 2026-11-20 | M3-VTE++ | **Real cause of the htop/vim freeze after enlarging FOUND and fixed** (log `/tmp/harakats.log`: output batches stop abruptly after ~100 messages, before/after the resize): russh 0.65 delivers every channel message to `Handler::data` **and** to the `Channel`'s internal queue (bounded at `channel_buffer_size` = 100, `chan.send().await` in the session loop, `client/encrypted.rs:611`). Nobody read that queue → at the ~100th message the SSH loop blocked permanently (no more output, `window_change` never processed). Fix: `channel.split()`; read half drained by a dedicated task, write half (`ChannelWriteHalf`) stored in `SessionEntry`. Regression test `many_small_messages_do_not_stall_session` (400 one-byte echoes + resize; the version without drain blocks). Verified: fmt, clippy `-D warnings` (workspace), 13 `haraka-ssh/ssh` tests green. The previous fixes (idle resize, ordered channel, lock outside awaits, coalesced feed) remain valid but did not address this cause |
| 2026-11-20 | M3-VTE++ | **User validation**: htop/vim work after enlarging (build with the russh channel drain). Freeze closed. SFTP: no panel yet (planned M5) |
| 2026-11-20 | M4 | **Clipboard and policies — verified green**: OSC 52 **intercepted in the backend** (streaming extractor with inter-chunk carry, BEL/ST terminators, common to VTE/xterm — sequences never reach the terminal); inherited policy `disabled/l2r/r2l/bidirectional` (effective + provenance); write subject to `r2l` + `osc52_max_bytes` (100 KB), **query (remote reading of the local clipboard) subject to `l2r` and always audited**; multiline/suspicious **paste confirmation** with exact preview (␛␍␊), anti-pastejacking (bracketed-paste markers stripped at the edges only), native GTK dialog (VTE) + web (xterm), input order preserved; optional **copy-on-select** (VTE selection-changed + xterm onSelectionChange), native primary selection; secret TTL unchanged (M2). **Documented deviation**: file copy-paste + drag & drop → M5 (depends on the SFTP panel). Tests: 14 osc52 (split chunks/oversized/query/invalid base64), 6 paste, policy matrix, 5 mirror vitest. **92 Rust tests, 14 vitest, 0/0**, packages 6.00 MiB |
| 2026-11-20 | M4-fix | **VTE paste fixed** (user feedback: confirmation dialog that does not disappear, Cancel redisplay it; Ctrl+Shift+C/V with no effect). Causes: ① confirmation was triggered on VTE's `commit` signal, hence on ANY input containing a control (arrows, Escape) and on each piece of a paste (bracketed-paste markers `ESC[200~`/`ESC[201~` + text = 3 `commit`s → 3 cascading dialogs); ② VTE does not wire Ctrl+Shift+C/V (the application must). Fix: every paste (Ctrl+Shift+V, Shift+Insert, middle click = PRIMARY, "Paste" menu) goes through `paste_from` (GTK clipboard → single confirmation if multiline/suspicious → `SessionCmd::Paste`); copy Ctrl+Shift+C / Ctrl+Insert; keystrokes are never confirmed; bracketed-paste mode tracked in the output stream (DECSET 2004) and internal markers stripped from the pasted content (anti-pastejacking); without bracketed paste, `\n`→`\r` like VTE. Verified: fmt, clippy `-D warnings`, workspace tests green. Manual test to redo: Ctrl+Shift+V of a multiline text (1 dialog, Cancel = nothing sent), arrows/Escape in vim without a dialog |
| 2026-11-20 | M3-VTE refactor | **Refactoring `vte.rs` (763 lines, one 240-line function with 8 parameters) → module `src-tauri/src/vte/`**: `ffi.rs` (libvte/pango FFI + safe `Term` wrapper, the module's only `unsafe`, documented invariants), `handle.rs` (`VteWindow`, registry, single helper `on_main_if_alive`), `window.rs` (assembly), `clipboard.rs` (shortcuts, middle click, menu, paste), `resize.rs` (initial PTY + debounce), `dialogs.rs` (auth, host key, paste confirmation: single `modal` construction + `on_response` answer), `mod.rs` (API + `VteOptions`). Behavior unchanged, public API unchanged except `open_window(…, VteOptions)`; no more `#[allow(clippy::too_many_arguments)]`. Verified: fmt, clippy `-D warnings`, workspace tests green |
| 2026-11-20 | M2-fix | **Clickable favorites** (user feedback: the FAVORITES group created by the star was inert): the Favorites and Recents rows now have the same actions as the tree — double-click / Enter = connect, ▶ Connect, ✎ Edit, ☆/★ remove/add favorite (yellow star when active, also in the tree). svelte-check 0/0, clean ESLint, vitest 14/14 |
| 2026-11-20 | M5 | **SFTP and transfers shipped**: `haraka-ssh` gains `sftp` (listing, mkdir, rename, recursive rm, recursive chmod, links, stat), `transfer` (queue, parallel workers 1–8, progress/throughput/ETA, pause/resume, **`.part` resume with trailing-64-KiB check**, recursive, rights/dates, conflicts, links ignored/followed, size verified) and `osc7` (current-directory following, sequences split across batches). Same SSH connection as the terminal (`SessionManager::sftp`, second channel, single-flight opening, invalidation on fatal error). **Critical fix**: `SshHandler::data` did not filter the channel → SFTP bytes would have been injected into the terminal; now only the shell channel feeds the display (test `sftp_bytes_never_reach_terminal`). `SSH_FXP_SYMLINK`: OpenSSH expects (target, link) — order observed on sftp-server. Tauri: 19 commands (`sftp_*`, `local_*`, `transfer_*`) + events `transfer://event`, `sftp://cwd`, `sftp://open`; SFTP button in the VTE status bar. UI: `SftpPanel`/`FilePane` (two panels, internal and OS drag & drop, CRUD actions, transfer list), active-transfers badge. **Tests** (real OpenSSH interop): CRUD/links/chmod, round trip with rights+mtime, **110 MiB: pause → resume → connection cut → new session → resume → identical file**, 105 MiB upload pause/resume, corrupted `.part` → restart from 0, recursive tree, 6 jobs / parallelism ≤ 3, skip/overwrite conflicts, permission denied / invalid paths; osc7/sftp/transfer unit tests; vitest `sftp.test.ts` |
| 2026-11-20 | M5+ | **User feedback**: ① **main window in windowed mode** (no more default maximization), size + maximized state memorized in `window_state.json` on close and restored (window created invisible then shown after restoration, no flicker; bounds 940×600…10000); ② **one-click terminal following** ("Enable following", MobaXterm-style): `sftp_enable_follow` types an idempotent bash/zsh line into the shell (OSC 7, spaces and `%` encoded — verified under bash and zsh); opt-in setting `sftp_follow_terminal` to inject it on every (re)connection; the paste-ready snippet remains available; ③ **editable path bar** in both SFTP panels (`~`, relative, `..` resolved, Escape). Verified: fmt, clippy `-D warnings`, `window_state` tests, vitest 22/22 |
| 2026-11-20 | M5++ | **User feedback**: ① **Recents** section removed (tree and welcome); ② **non-modal SFTP**: the panel becomes an integrated view of the central area (mounted once, hidden/shown without state loss, OS drops ignored when hidden), the tree and toolbar remain usable; ③ **invisible following activation**: redirecting output to /dev/null would have changed nothing (the displayed line is the terminal's ECHO, not an output) — the echo is therefore concealed (SGR 8 before sending) then erased by relative moves (`ESC[nA`, `CR`, `ESC[J`, n = occupied lines computed from the VTE cursor column and width) inserted right after the first received OSC 7 (`Osc7Scanner::first_end`), 4 s guardrail, hook made idempotent AND always announcing (final OSC 7). **Validated in a real pty + emulator (pyte) under bash and zsh, prompt at the bottom of the screen with scrolling**: a single remaining prompt, OSC 7 emitted on `cd`. Verified: fmt, clippy `-D warnings`, osc7 tests, vitest 22/22, svelte-check 0/0 |
| 2026-11-20 | M5+++ | **User feedback**: ① footer banner (version, vault id, data folder) **removed** — errors go into a floating notification; ② **About menu** (`about_info` + `About.svelte`) with application info and **MIT license** (`LICENSE` created, `license = "MIT"` in Cargo, `bundle.license/licenseFile` in `tauri.conf.json`); ③ **bottom of the SFTP panel hidden** (Send/Download buttons… behind the footer): footer removed + scrollable central area (`.content { overflow-y: auto }`); ④ log checked: no application error (expected "connection refused" failures toward a host without sshd, reconnection with 2/4/8/16/30 s backoff); the `close session: session not found` warning when closing a window whose reconnection failed was harmless → downgraded to debug |
| 2026-11-20 | M5++++ | **User feedback**: ① "Enable following" button **removed**: the "Follow terminal" checkbox activates it itself on click (concealed injection, once per session as long as no OSC 7 has been seen); ② **SFTP view reorganized and aligned**: cause of the misalignment = global rule `main { max-width; margin: 0 auto; padding: 24px 16px 64px }` applied to `<main class="content">` (neutralized); panel as a card aligned with the tree (top and bottom), full-height flex, compact buttons in each panel's header, collapsed transfer options, transfers at the bottom (150 px max, scrollable), no more whole-view scrolling. Verified by **real visual rendering** (headless Chromium + Tauri mockup) at 1400×850 and 980×620; this rendering also revealed an orphan CSS brace (an unclosed `@media` swallowed all following rules) — fixed; ③ log: SFTP session opened, no error |
| 2026-11-20 | i18n | **Complete internationalization** (feedback: poorly rendered About button, English still full of French): ① languages **fr, en, it, de, es, pt**; ② **one JSON per language** (`ui/src/locales/`, 470 keys), automatic discovery, fallback language → English → French, `{parameters}`, plurals `_one/_other` (Intl.PluralRules), dates and numbers per language; ③ **single source for the web AND native GTK/VTE** (`native.*` keys, `src-tauri/src/i18n.rs`): menus, dialogs (host key, paste, authentication), status bar and messages written into the terminal; ④ **111 backend error translation rules** (`errors.ts`, plugged into `message()`); ⑤ language memorized for the unlock screen (localStorage) + system language detection; ⑥ **automatic guardrails**: key/parameter parity across all languages, no hardcoded visible text in components, all languages declared on the Rust side; ⑦ **"About"** button without the ℹ glyph. Parallelized work (5 extraction agents by interface area, 5 for translation). Verified: fmt, clippy, Rust tests, svelte-check 0/0, ESLint, vitest 75/75, real visual rendering in German and English |
| 2026-11-20 | M6 | **Remote editing shipped** + **single terminal engine (VTE)**. Editing: `haraka-ssh::edit` (download to `tmp/<uuid>/` 0700/0600, mtime+size+SHA-256 reference, **explicit conflict** overwrite/reload/compare, send via remote temporary + replacement with backup/restore, bytes and CRLF/LF preserved, external editor in argv WITHOUT shell watched by `notify` with 400 ms debounce and hash-based anti-loop, cleanup on close / lock / startup); 9 `edit_*` commands + `edit://event` event; CodeMirror 6 UI lazy-loaded (`EditorView`, `CodeEditor`, `MergeCompare`, non-modal view), Edit / Edit externally buttons, double-click on a remote file, External editor setting. **Real OpenSSH interop tests**: 0700/0600 rights of the temporary, round trip with preserved rights and no `.tmp/.bak` residue, `touch` ≠ conflict, conflict then the 3 resolutions, deleted file, CRLF and non-UTF-8 byte for byte, refusal > 20 MiB, **concurrent reader never facing a partial file**, fake external editor (auto send, conflict, no loop), startup purge; argv/line-ending unit tests. **xterm engine removal** (user request): engine choice removed from Settings, Terminal/SplitPane/tabs/paste components and `@xterm/*` dependencies removed (bundle 580 → 316 KB), IPC commands `session_open/reattach/write/resize`, `clipboard_read_text`, `terminal_engine` field, `detach-*` windows and webview-creation capability removed. i18n: 66 new keys × 6 languages |
| 2026-11-21 | M6-fix | **"Reload" did not display the reloaded content** (user feedback; the remote file was indeed up to date, and the external editor saved correctly). Cause: "Reload" replaces the store content then increments the revision, which unmounts/remounts the CodeMirror editor; on unmount the OLD instance sent its old text one last time (`flush`) and overwrote the freshly loaded content just before the remount — the guardrail read the reactive revision, still stale during unmount (Svelte reactivity batch). Fix: NON-reactive revision mirror (`currentRev`) updated immediately, guardrail in `EditorView` (send ignored if the instance's revision ≠ current revision), revision frozen at mount in `CodeEditor` for the state cache. Same defect fixed for the refresh after an external-editor save. Reproduced then verified **in a real browser** (real component + simulated IPC); tests: `edit.svelte.test.ts` (store), svelte-check 0/0, ESLint, vitest 77/77 |
| 2026-11-21 | M7 | **v1 finalization: tunnels, multi-exec, macros, logging, imports, packaging.** *Tunnels* (`haraka-ssh::tunnel`) L/R/D + homegrown SOCKS5, 🔀 view + spec editor, idempotent auto-start on every (re)connection (`post_connect.rs`), stop on session close / lock; **9 real OpenSSH interop tests** (banner via -L, SOCKS5 domain/IPv4/IPv6 + BIND/UDP refusal, -R port 0 and fixed, conflicts, 100 connections, 20 MiB SHA-256, port release) + unit tests. *Multi-exec* (`haraka-ssh::multiexec`) byte-for-byte broadcast without loop, native indicator + banner, confirmation, auto-disable; **2-real-session interop test** (200 lines in 37-byte pieces, same order). *Macros*: complete UI, encrypted content revealed only on an audited action, send with preview/confirmation, native right-click submenu, startup snippet ("confirm" macro ignored). *Logging* opt-in (`haraka-core::sessionlog`, 0600/0700, ANSI stripped, 50 MiB cap, after OSC 52 extraction). *Imports* ssh_config / Remmina / MobaXterm (`haraka-core::import`, backend-side preview, keys imported into the vault, report, **deterministic fuzz 500 entries without panic**, anonymized fixtures). *Packaging/CI*: metadata (publisher, MIT license, description), CI checks `rpm -qp --requires` (vte291, webkit2gtk4.1), package license and files, npm audit; **v1 format fixtures frozen** (xtask no longer regenerates them without `--force`); §5.13 acceptance checklist; final README. i18n: 196 new keys × 6 languages |
| 2026-11-21 | Security | **Hardening (5 points)**: ① changed host key = **hard block** (no acceptance button; replacement via Settings → Known hosts → Forget, `knownhosts::forget`, audit); ② vault: directory fsync, verification re-read, backup rotation only if the vault is valid, integrity check on unlock (valid/corrupted backups, restoring a corrupted one refused); ③ session logs **encrypted by default** (`.log.enc`, XChaCha20-Poly1305, subkey derived from the DEK), reading/deletion/purge in Settings, 30-day retention; ④ editing temporaries in `$XDG_RUNTIME_DIR` (tmpfs), overwritten then deleted; ⑤ SSH agent: status and identities visible (FIDO2 `sk-` keys via the agent). Tests: 2 new (encrypted log/purge, store), host-key test adapted |
| 2026-11-21 | Versions | **New SemVer versioning aligned with milestones**: the M7 + hardening state becomes **0.7.0** (tag `v0.7.0`), SFTP alone will be 0.8.0, `1.0.0` = §5.13 acceptance validated. **Single source**: `[workspace.package] version` of `Cargo.toml`; `cargo run -p xtask -- version check\|sync\|set X.Y.Z` aligns `package.json` ×2 and `tauri.conf.json` (check in CI). **Git hash** in "About" (`v0.7.0 · 7d2e042`, `-dirty` suffix if the tree is modified) and `harakats --version` (build.rs → `HARAKA_GIT_HASH`) |
| 2026-11-21 | 0.7.1 | **Version 0.7.1**: creator **LazySysAdmin** (README, "About", `LICENSE`, package publisher), `README.md` in English by default + `README.fr.md`, branch strategy (§5.12ter: `main` stable / `dev` / `feat/*`) |
| 2026-11-21 | 0.8.0 | **Version 0.8.0** (SFTP alone moves to 0.9.0): session bar + SFTP linked to the active session (automatic opening/connection), tabbed terminals (experimental option), multi-exec in a floating panel with existing-macro sending, terminal output colorization (fixed rules), "Sessions" button removed |
| 2026-11-21 | Licenses | **License compliance (MIT audit → implementation)**: ① `THIRD-PARTY-LICENSES.md` generated by `cargo run -p xtask -- licenses` (**cargo about** for Rust + **pnpm licenses list --json** for npm, complete deduplicated texts, real versions) — embedded at compile time (`include_str!`, `third_party_licenses` command), **"Third-party licenses"** button in About (6 languages), bundled in the packages (`bundle.resources`, verified with `rpm -qpl`); ② **`cargo deny check licenses` guardrail** (`deny.toml`, permissive MIT-compatible allowlist including Apache-2.0 WITH LLVM-exception; GPL/AGPL/unknown = failure) in CI (core job) + local sequence; ③ container image: cargo-deny + cargo-about (**`cli` feature required ≥ 0.9**); ④ `about.toml`/`about.hbs`, build_now.md sequence and CI aligned. Audit: no MIT conflict (russh/aws-lc Apache/ISC → notices kept; GTK/VTE/WebKit LGPL in dynamic linking only). Verified green: **191 Rust tests, 111 vitest, 0/0, `licenses ok`**, 0.8.0 packages 7.92/7.93 MiB |
| 2026-11-21 | 1.0.0 | **Version 1.0.0 — first V1**: the project owner's validation of the interface (connections column resizable 200–420 px, tabs on the right with a state dot, on-demand SFTP panel following each shell's folder, single ☰ menu, quick connect bar and "New" button removed, "Close all sessions when the vault is locked" option with hidden terminals, green/red server status, ⓘ help tooltips in Settings, fix for the panel that used to disappear). English translations of the docs (`*.en.md`) up to date with 0.8.0 only |
| 2026-11-21 | 1.0.1 | **1.0.1 fixes and polish**: tree/terminals separator with no flicker at the 200/420 px limits (bound applied in the `notify::position` signal, native floor), useless "Root" drop zone removed under the tree (moving goes through "Move to…"), "(experimental)" mention removed from tabs mode, CSS design tokens (radii, shadows, sizes, spacings, semantic colors), global keyboard focus ring, `prefers-reduced-motion`, light/system theme contrasts verified by test (vitest `theme-contrast`) |
| 2026-11-21 | 1.0.2 | **1.0.2 security and slimming** (qwen audit): known_hosts **fail-closed** (unreadable file = connection refused, corrupted line = changed key), `@revoked` keys refused (imported from the system known_hosts), clipboard **cleared on lock** and secret copy **excluded from Klipper's history** (`x-kde-passwordManagerHint` indicator), copies of the vault key, keyboard-interactive answers and the copied secret erased from memory, atomic write to a random file name with exclusive creation, data folders created 0700, extended permission checks (backups, logs, audit, known_hosts); **size**: binary 21.2 → 14.6 MB (fat LTO, opt-level s), package 7.97 → 5.97 MiB, removal of the 2 s polling of the session list (`session://state` event) |