# ROADMAP — HarakaTS v1 (milestones M1 → M7)

> Phase 0 — development plan. To be validated before any code is written.
> Rule: every milestone is **compilable, tested, delivered with its summary** (done / remaining /
> deviations from the plan). No "half" milestone: the main acceptance criterion
> (export A → import B) runs in CI **from M1** and stays green afterwards.

## Definition of Done (every milestone)

- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo audit` (blocking vulnerabilities = 0), `cargo test --workspace` green.
- `pnpm lint` (ESLint 9 + typescript-eslint), `pnpm check` (strict tsc), `pnpm test`
  (vitest) green as soon as the UI exists (M2+).
- Milestone tests executed and listed in the summary; GitHub Actions CI up to date.
- MODE_D_EMPLOI.en.md updated (new usable features).
- No secret in logs/IPC/panics (targeted review at every milestone).
- Git: commits/push **in the hands of the project owner** (I commit nothing).

## Environment (cross-cutting, from M1)

- Podman dev container (`packaging/Containerfile`, Debian bookworm): Rust stable
  (≥ 1.90), Node 22, pnpm, webkit2gtk-4.1-dev, GTK3, libsoup3, openssh-server (interop
  tests), dbus. The host OS is not modified by development.
- Host: only the **runtime** libs (installed by the user, see MODE_D_EMPLOI.en.md) to run
  the produced binary.
- Dual test SSH server: in-process (russh server, hermetic) + real openssh (dev container /
  CI runner, port 2222, disposable config and keys in `tests/`).

---

## M1 — Vault, crypto, export/import (the heart of the project)

**Scope**: `haraka-core` (vault, store, export, audit) + minimal `src-tauri` + minimal UI
(create/unlock/lock screen, vault settings, export/import dialog). No usable connection tree
yet (M2).

- Vault: DEK/KEK, calibrated Argon2id (0.5–1 s), double-layer XChaCha20-Poly1305, canonical
  AADs, atomic writes 0600/0700, rotating backups, corruption restore.
- Master password change (re-wrap only), opt-in **non-blocking** keyring cache
  (KWallet/Secret Service via keyring 3.6.2 vendored; absence → silent fallback).
- Auto-lock: inactivity + `PrepareForSleep` + session lock (zbus), zeroize.
- `.hrkx` export (everything / folder / no secrets, distinct passphrase) and import
  (preview, keep/replace/duplicate conflicts, local DEK re-encryption, transactional).
- Local audit log.
- **Complete EXPORT_FORMAT.en.md §9 normative tests**, including:
  - headless A→B round trip (two data dirs, two masters);
  - bit-by-bit tampering (header, nonce, payload, tag) → rejection;
  - secret permutation (entry/field) → AAD rejection;
  - versioned fixtures.
- CI: GitHub Actions workflow (fmt/clippy/audit/test core + minimal UI).

**Acceptance**: create a vault A with dummy credentials, export, import into virgin B with
another master, recover all secrets identically; any bundle alteration is rejected; without
KWallet, everything works.

## M2 — Connection model, tree, inheritance, shared credentials

**Scope**: complete store (folders/connections/credentials/macros/recents/ui_state),
management UI.

- Folder/subfolder tree: drag & drop, tags, colors, icons, sorting.
- Property inheritance (fold defaults → folders → connection), "effective value +
  provenance" preview, explicit override; cycles forbidden.
- Decoupled credentials: CRUD, referenced by N connections, `needs_reentry`, explicit
  reveal/copy with N s clipboard clearing, audit.
- Quick-connect bar `user@host:port`, favorites, recents, fuzzy search Ctrl+K (fuzzysort),
  FR/EN i18n.
- Complete settings (default policies, auto-lock, KDF re-calibration, locale…).
- Finalized lock/unlock screen; detachable tabs: multi-window preparation (capability).
- **Tests**: inheritance resolution (case matrix), referential integrity (deleting a
  referenced credential), `schema_version` migration, atomic store CRUD, vitest
  tree/inheritance components.

**Acceptance**: a folder defines user/port/credential/jump/clipboard policy; child
connections inherit or override; editing a credential updates every connection; the tree is
fully operable by keyboard and mouse.

## M3 — SSH terminal

**Scope**: `haraka-ssh` (session, auth, host keys, jump, keepalive/reconnect) + terminal UI.

- Auth: password, key (ed25519/ecdsa/rsa, encrypted keys accepted, vault passphrase),
  two-step keyboard-interactive (2FA/OTP, prompt loop), SSH agent (`SSH_AUTH_SOCK`).
- Host keys: TOFU + SHA256 fingerprint, internal known_hosts + system import (hashed),
  change → blocking alert, audit.
- Chained jump hosts (`connect_stream` over `direct-tcpip`), hop inheritance.
- xterm 6 terminal: tabs, H/V splits, window detachment, fit/search/web-links/
  unicode11/webgl (+ DOM fallback, contextloss), resize (`window_change`), batched
  `Channel<Vec<u8>>` stream, configurable encoding (encoding_rs).
- Keepalive (`send_keepalive`), latency (`send_ping`), auto-reconnect backoff, server
  banner, UI auth prompts.
- **Tests**: in-process russh server (auth success/failure/partial, multi-prompt
  keyboard-interactive, unknown/changed host key, 2-hop jump, resize, keepalive) + container
  openssh interop (real auth, PTY, shell). Reconnection: simulated network cut.

**Acceptance**: open a session to the test server with every auth method, through a jump
host, with TOFU then a key-change alert; functional tabs, splits and detachment; no
regression of the A→B test.

## M4 — Clipboard and policies

**Scope**: inheritable `disabled/l2r/r2l/bidirectional` policies, OSC 52, paste protections,
copy-on-select, middle click, files between panels.

- OSC 52 handler (xterm parser) subject to the policy + size limit (default 100 KB);
  verification of xterm 6's native OSC 52 behavior (ours stays in control).
- Bracketed paste; multiline confirmation + suspicious content detection (control/escapes)
  with exact preview; optional copy-on-select; Linux primary selection (arboard);
  Ctrl+Shift+C/V.
- **File** copy-paste local panel ↔ remote panel (internal format); OS → app drag & drop
  (Tauri events) → upload. Drag-out: documented limitation.
- Automatic clipboard clearing (copied secret) + Klipper history warning.
- **Tests**: matrix of the 4 policies × 2 directions × inheritance, oversized OSC 52 →
  refusal, suspicious paste → confirmation, vitest detection logic; documented manual test
  (KDE/Klipper).

**Acceptance**: a folder's policy applies to child connections; OSC 52 respects policy and
size; a multiline paste asks confirmation; copying a secret clears it after N s.

## M5 — SFTP browser and transfers  ✅ shipped (2026-11-20)

> Deviations from the plan: in-process russh server not used for errors — everything goes
> through real OpenSSH interop (real permission denied, invalid path); simulated "disk full"
> not tested; `check-file` hash and `chown` not shipped; non-pipelined transfers; folder
> permissions not preserved. See MODE_D_EMPLOI §5.5 (v1 limits).

**Scope**: SFTP panel on the same connection (`into_stream` → `SftpSession`), complete
transfer engine.

- Tree + list, hidden files, sorting, search; rename, delete, mkdir, chmod/chown,
  properties, copy path, symlinks.
- "Follow the terminal's directory": OSC 7; one-click activation (line typed into the shell,
  opt-in, audited) or paste-ready snippet; optional automatic injection on connection (setting).
- Transfers: queue, progress/throughput/ETA (Channel), pause/resume, **resume after
  interruption** (`.part` + offset + seek), parallelism (3 default, 8 max), recursive,
  preserved permissions/dates, integrity (size; hash if `check-file` extension; opt-in full
  re-read), symlink policy.
- **Tests**: container openssh interop — large file (100 Mi+) interrupted then resumed
  (identical bytes, local hash), parallel transfers, recursive folder with permissions/dates,
  pause/resume, in-process russh server for error cases (permission denied, simulated disk
  full, invalid path).

**Acceptance**: an interrupted transfer resumes and produces an identical file; the SFTP
panel requires no second authentication; remote CRUD operations work.

## M6 — Remote editing  ✅ shipped (2026-11-20)

> Deviations: remote replacement via `.bak` backup + rename (no `posix-rename` in russh-sftp);
> xterm engine removal (VTE only) done in this milestone. See ARCHITECTURE §11.

**Scope**: integrated CodeMirror 6 + external editor, private temp, watch, automatic upload,
conflicts.

- Download → `data/tmp/<uuid>/` (0700/0600), extension-based highlighting, search, LF/CRLF,
  tabs; configurable external editor (direct argv, backend).
- `notify` watcher + debounce; atomic upload (remote temp + rename + metadata).
- Conflict: mtime/size then SHA-256 hash; overwrite/reload/**compare** dialog
  (`@codemirror/merge`); temp purge on close and at startup.
- **Tests**: conflict scenario (simulated remote modification between download and save),
  atomic upload (no partial file visible), crash cleanup (startup purge), external editor
  (dummy command), preserved line endings.

**Acceptance**: edit a remote file, save → automatic upload; if the remote changed in the
meantime, the conflict is detected and the 3 choices work; no temp residue after closing.

## M7 — Tunnels, multi-exec, imports, packaging (v1 finalization)  ✅ shipped (2026-11-21)

> Deviations: `remmina.pref` (Remmina password decryption) not implemented — `needs_reentry`;
> ssh_config `Match` / `ProxyCommand` flagged as unsupported; MobaXterm embedded keys not
> imported; tunnels: no supervision of an errored tunnel, non-loopback bind warned but not
> blocked; no physical virgin machine in CI (manual acceptance §5.13 MODE_D_EMPLOI).

**Scope**: everything else from the prompt + delivery.

- L/R/D tunnels: graphical manager (per-connection specs, live, auto_start), minimal SOCKS5
  (CONNECT, loopback by default), remote forward via `server_channel_open_forwarded_tcpip`,
  bind/GatewayPorts warnings.
- Multi-exec (tab selection, fan-out, permanent indicator, confirmation).
- Macros/snippets (encrypted content, send confirmation).
- Opt-in session logging (0600, echo warning).
- Imports: `~/.ssh/config` (Include, ProxyJump, IdentityFile → keys into the vault, wildcard
  hosts → folder defaults), `.remmina` (INI, `needs_reentry`, experimental best-effort with
  provided remmina.pref), `.mxtsessions` (INI-like, `needs_reentry`); import reports, never a
  crash.
- Packaging: **.deb + .rpm** (tauri bundler; AppImage removed, user decision of 2026-11-20),
  RPM dependencies verified via `rpm -qp --requires`, targets Fedora ≥ 42 / EL10 via EPEL /
  Mageia (**EL9 excluded**, see ARCHITECTURE §16), icons, `com.harakats.desktop` metadata,
  portable mode (`portable.marker` / `--data-dir`).
- Complete CI: fmt/clippy/audit/workspace tests + lint/tsc/vitest + openssh interop +
  **mandatory A→B test** + package build.
- Final documentation: complete MODE_D_EMPLOI.en.md, reviewed SECURITY.en.md, frozen v1
  EXPORT_FORMAT.en.md, up-to-date README.
- **Tests**: L/R/D tunnels through the openssh container (real connection via tunnel, SOCKS5
  curl), multi-exec (2 sessions, same bytes), imports on real anonymized fixtures (complex
  ssh config, .remmina, .mxtsessions), packages installed and launched on a clean machine
  (MODE_D_EMPLOI checklist).

**v1 acceptance**: on a virgin Linux machine (KDE or without keyring), install the .deb
(Debian/Ubuntu) or the .rpm (Fedora / EL10+EPEL), create a vault, import a config, open SSH
sessions with tunnels and SFTP, edit remotely, export, re-import elsewhere without re-entry.
All CI tests green.

---

## Identified risks and mitigations

| Risk | Mitigation |
|---|---|
| Moving russh/russh-sftp API (0.x) | pinned versions + committed Cargo.lock (by you); docs.rs re-verification at every milestone; in-process test server to decouple from network hazards |
| aead 0.6 / rand_core 0.10: obsolete old examples | ARCHITECTURE §2.4 documents the alignment; crypto tests from M1 |
| xterm 6: reworked viewport/scrollbar, canvas removed | DOM fallback planned; M3 visual tests; strict addon pinning |
| xterm 6 native OSC 52 conflicting with our policy | M4 verification, master handler, native option disabled if needed |
| WebKit/GPU in the webview (Wayland KDE) | DOM renderer fallback; test on a real machine from M3 |
| russh: bounded internal `Channel` queue (100) → SSH loop blocked if not drained (htop freeze, observed 2026-11-20) | `split()` channel + drain task; test `many_small_messages_do_not_stall_session`; to keep for every new channel type (SFTP M5, tunnels M6) |
| Host glibc vs build container | Debian bookworm build (glibc 2.36) → compatible Debian 12+/Ubuntu 22.04+ and Fedora/EL10 (newer glibc); documented |
| RPM targets without webkit2gtk4.1 (EL9) | documented v1 targets: Fedora ≥ 42, EL10 via EPEL, Mageia; **EL9 excluded** (tauri#9039); RPM deps verified via `rpm -qp --requires` at M7 |
| SFTP transfer resume (server seek) | real interruption test from M5; if the server does not support the expected offset → resume from 0 with a warning |
| Scope drift | this document + deviation summary at every milestone end; any unlisted feature = deferred post-v1 by default |

## Order and dependencies

```
M1 (vault) ──▶ M2 (model/UI) ──▶ M3 (SSH) ──▶ M4 (clipboard)
                                   │
                                   ├──▶ M5 (SFTP) ──▶ M6 (editing)
                                   └──▶ M7 (tunnels/multi-exec/imports/packaging)
```

M4 depends only on M3; M5/M6 depend on M3; M7 closes. The A→B test (M1) is replayed at every
milestone in CI.

## After v1 (0.N versions)

| Version | Content | Status |
|---|---|---|
| 0.7.0 / 0.7.1 | M7 + security hardening (blocking host key, vault integrity, encrypted logs, tmpfs, agent/FIDO2), versioning, creator | shipped |
| 0.8.0 | Session bar, SFTP linked to the active session, tabbed terminals (option), floating multi-exec + macros, output colorization | shipped |
| 0.9.0 | SFTP alone (without terminal), "Test connection", non-interactive then interactive authentication | planned (`feat/sftp-only`) |
| 1.0.0 | §5.13 acceptance fully validated | upcoming |