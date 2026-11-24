# HOW IT WORKS — The SSH connection in HarakaTS

🇫🇷 [Version française](FONCTIONNEMENT_SSH.md) · 🇬🇧 English

> Explanatory document (read-only, no code change). Describes the full path of an SSH
> connection, from clicking ▶ to display in the native terminal, as implemented in
> `crates/haraka-ssh/src/session.rs` and `src-tauri/src/sessions.rs` (v1.0.8).
> Complements: [ARCHITECTURE.en.md](ARCHITECTURE.en.md) §7, [SECURITY.en.md](SECURITY.en.md) §7.

## 1. Overview

```
click ▶ / double-click / Ctrl+K
   │
   ▼
[1] Backend resolution (effective config + credentials)
   │
   ▼
[2] Native VTE window opens → real dimensions (cols×rows)
   │
   ▼
[3] TCP → SSH handshake → [4] host key verification → [5] authentication
   │        (chain of hops if jump hosts)
   ▼
[6] Shell channel: PTY + shell, split into read/write
   │
   ├── output: Handler::data → queue → decoding/OSC52/log → VTE (display)
   ├── input: VTE keyboard → ordered task → SSH write
   └── resize: window size → window_change → server PTY
```

A connection = **one russh SSH channel** (with nested hops if needed), authenticated on the
backend side, verified against MITM, whose shell channel is continuously pumped to a **native
VTE** terminal. An SFTP channel and tunnels can ride on the same connection, **without a second
authentication**.

## 2. The steps in detail

### [1] Backend-side resolution
`build_open_request` computes the **effective config** by inheritance (system → groups →
connection: host, port, user, clipboard policy, keepalive, encoding…) and the **authentication
material** via `materials_for`, in the effective order (agent, key, password,
keyboard-interactive).

**Secrets never leave the backend**: the interface only receives metadata (sanitized DTOs).
The material is resolved from the unlocked vault, in memory.

### [2] Native window
A **GTK+VTE** window is created (`src-tauri/src/vte/`) and the server PTY is opened only at its
**real dimensions** (cols×rows). Otherwise htop/vim/tmux are drawn for a wrong size (commands
behind the prompt, disordered top).

### [3] Transport
- TCP connection to `host:port` (`russh::client::connect`, or `connect_stream` on top of a hop),
  then the SSH handshake and algorithm negotiation (kex, ciphers, host keys).
- **Jump hosts**: connect to the bastion → `channel_open_direct_tcpip(next_host)` →
  `into_stream()` → new nested SSH connection (`connect_stream`), up to the target. Each hop
  has its own credential (inheritance included). `ProxyJump` imported from ssh_config maps
  directly to this chain.

### [4] Host key (anti-MITM)
`Handler::check_server_key` → `verify_host_key`:
- **SHA256** fingerprint (OpenSSH format) compared against the internal `known_hosts`;
- **known and identical** → accepted;
- **unknown** → **TOFU**: dialog showing the fingerprint, "Accept and remember" or "Reject";
  300 s timeout → refusal;
- **changed** → **hard block**: no accept button is offered; replacement only via Settings →
  Host keys (internal known_hosts) → **Forget** (audited), then a new TOFU;
- **unreadable file** (I/O error, invalid bytes) → **refusal** (fail-closed); a corrupted line
  for the host counts as a changed key. `@revoked` entries are refused without a prompt.

No automatic decision: the user decides explicitly (except refusal by default).

### [5] Authentication
`authenticate` tries the materials in the effective order:
- **SSH agent** (`SSH_AUTH_SOCK`, FIDO2 `sk-` keys included; **no agent forwarding**);
- **private key** (decrypted with the vault passphrase; RSA hash negotiated via
  `best_supported_rsa_hash`);
- **password**;
- **keyboard-interactive** (2FA/OTP): loop of prompts displayed on screen; automatic prefill
  is limited to the "password" prompt (never sent to an OTP prompt).

**Fallback and errors.** When every configured method fails, `authenticate` records the methods
the server still offers. If it accepts a password, a "Password for user@host" prompt opens
(then keyboard-interactive if offered) instead of a bare failure. The final error lists the
methods **tried** and the methods **offered by the server** (e.g. RouterOS rejects ed25519 keys:
it offers `publickey,password`). An authentication refusal is never replayed by automatic
reconnection.

The material is **zeroized** after use and on vault lock. OTP answers are `Zeroizing<String>`.

### [6] Shell channel
`channel_open_session` → `request_pty(term, cols, rows)` → `request_shell(true)`.

The channel is **split** (`channel.split()`):
- a **drain task** consumes the read half (`read_half.wait()`) — mandatory: russh's internal
  queue is bounded (`channel_buffer_size` = 100); without draining, the session loop blocks at
  the ~100th message (this was the cause of the htop freeze);
- the write half (`ChannelWriteHalf`) serves `data_bytes` / `window_change`.

The **shell channel** id is recorded (`shell_channel`) so that `Handler::data` sends to the
terminal **only its data** — SFTP bytes from another channel must never appear in the terminal.

## 3. Continuous flows

### Output (server → screen)
`Handler::data` → mpsc queue → `spawn_output_forwarder`:
1. `encoding_rs` decoding (UTF-8 by default, configurable charset per connection);
2. **OSC 52** extraction (inherited clipboard policy; sequences never reach the terminal);
3. syntax colorization (fixed rules, never inside a fullscreen program);
4. session log (opt-in, encrypted, after OSC 52 extraction);
5. **16 ms / 32 KB batches** → `vte_terminal_feed` (native display, a single GTK send in flight).

### Input (keyboard → server)
VTE `commit` signal → `SessionCmd::Input` → **one ordered task per session** (encodes to the
connection's charset) → `channel.data_bytes`. Input and resize cannot cross or reorder.

### Resize
VTE `size-allocate` (read in **idle**, grid up to date) → `SessionCmd::Resize` →
`window_change` → SIGWINCH on the server side (100 ms debounce). The size is memorized
backend-side for reconnections.

### After connecting (directory tracking)
When the "follow terminal" option is on, HarakaTS may type a (bash/zsh syntax) line into the
shell that emits OSC 7 at each prompt. It is injected **only** if the server identifies as
**OpenSSH** (`SSH-2.0-OpenSSH…`) and the prompt does not end with `>`; MikroTik (ROSSSH), Cisco,
Dropbear/BusyBox and unknown identifications never receive it (it could freeze their CLI).
Manual activation remains available from the SFTP panel.

### Keepalive
russh keepalive (configurable interval per connection, 3 failures max before disconnect).

### Reconnection
Backoff 1→2→4→…→30 s, max 5 attempts, **only while the vault is unlocked**; re-authentication
from the backend memory cache. Final failure → "disconnected" state, Reconnect button.

### Locking
All sessions close, DEK and material zeroized, decrypted remote files wiped, secret clipboard
cleared (`x-kde-passwordManagerHint` indicator for Klipper).

## 4. Key security points
- Secrets resolved and kept **backend-side only**; explicit, audited reveal/copy.
- **Anti-MITM**: TOFU + hard block on change + fail-closed on unreadable file.
- **No agent forwarding** (a compromised remote server cannot use the agent).
- External editor launched in **argv without a shell** (no injection).
- SFTP/tunnel channels on the same connection, no re-auth; loopback bind by default.
- Details: [SECURITY.en.md](SECURITY.en.md) §7.

## 5. Reference files
| Element | File |
|---|---|
| Session manager, handler, auth, hops | `crates/haraka-ssh/src/session.rs` |
| Config + material resolution, forwarder, IPC commands | `src-tauri/src/sessions.rs` |
| Native VTE window/terminal | `src-tauri/src/vte/` |
| known_hosts verification | `crates/haraka-core/src/knownhosts.rs` |
| SFTP (2nd channel) | `crates/haraka-ssh/src/sftp.rs` |
| L/R/D tunnels + SOCKS5 | `crates/haraka-ssh/src/tunnel.rs` |
| OSC 52 / clipboard | `crates/haraka-core/src/osc52.rs` |
| Encrypted session log | `crates/haraka-core/src/sessionlog.rs` |