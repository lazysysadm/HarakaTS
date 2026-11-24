# HarakaTS

🇫🇷 [Version française](README.fr.md) · 🇬🇧 English

📚 English documentation: [Security](SECURITY.en.md) · [Architecture](ARCHITECTURE.en.md) · [User guide](MODE_D_EMPLOI.en.md) · [Export format](EXPORT_FORMAT.en.md) · [Roadmap](ROADMAP.en.md) · [Build now](build_now.en.md) · [How SSH works](FONCTIONNEMENT_SSH.en.md)

**Created by LazySysAdmin.**

Desktop remote-connection manager for Linux (SSH + SFTP in v1), in the spirit of
Remmina / MobaXterm, with one central requirement: **encrypted, portable secrets**.
An export re-imported on another machine recovers every password and key with no
re-typing — unlike Remmina, whose secrets are tied to the local keyring.

## Project status

**v1 delivered** (milestones M1 → M7, 2026-11-21). Portable encrypted vault with
export/import; inheritable connection tree; native GTK+VTE terminal; SFTP (two-pane
browser, resumable transfers); remote editing (CodeMirror 6 / external editor, conflict
detection); session bar, optional mRemoteNG-style terminal tabs with an on-demand SFTP panel, server
status dots, lock-without-disconnect option; L/R/D tunnels; multi-exec (floating panel), macros, session logging; output colorization; imports of `~/.ssh/config`,
Remmina and MobaXterm; UI in 6 languages (fr, en, it, de, es, pt); `.deb` and `.rpm` packages.
The code is built and tested in the Podman dev container
(see [MODE_D_EMPLOI.md](MODE_D_EMPLOI.md) §4; step-by-step tutorial: [build_now.md](build_now.md));
the v1 acceptance checklist is in §5.13 of the guide.

> The documentation is available in **French** (the original files, linked below) and in
> **English** (the `*.en.md` files, linked above).

## Installation

HarakaTS targets **Linux** (desktop with GTK 3). Prebuilt packages are attached to each release
(`.rpm` for Fedora/openSUSE/RHEL-like systems, `.deb` for Debian/Ubuntu-like systems). The package
manager pulls the runtime dependencies automatically (WebKitGTK 4.1, GTK 3, libsoup 3, VTE 2.91).

**Fedora / RHEL family**

```bash
sudo dnf install ./haraka-ts-<version>-1.x86_64.rpm
```

**Debian / Ubuntu family**

```bash
sudo apt install ./haraka-ts_<version>_amd64.deb
```

Then start **HarakaTS** from the application menu, or run `harakats` in a terminal.
`harakats --version` prints the version and the build commit.

- **First start:** choose a master passphrase to create the encrypted vault. It cannot be recovered:
  keep it safe. The OS keyring is optional and never required.
- **Your data** stays in `~/.local/share/harakats/` (vault, rotating backups, internal known_hosts,
  audit log). *Portable mode*: put a `portable.marker` file next to the binary, or pass
  `--data-dir <path>`, to keep everything in a `./data/` folder (USB key, synchronized folder).
- **Update:** install the newer package over the old one (`dnf upgrade ./…rpm` / `apt install ./…deb`);
  your data is untouched. If the version number is identical, use `sudo dnf reinstall ./…rpm`.
- **Uninstall:** `sudo dnf remove haraka-ts` or `sudo apt remove haraka-ts`. Your vault stays in
  `~/.local/share/harakats/` until you delete it yourself.
- **Upgrading from an older release** named `HarakaTS` (before the rename to `haraka-ts`): the new RPM declares `Obsoletes: HarakaTS`, so `sudo dnf install ./haraka-ts-<version>-1.x86_64.rpm` replaces it; your data is untouched.
- **Check a download:** compare `sha256sum <file>` with the checksum published with the release.
- **Build from source** (Rust + Node, inside a Podman container, nothing to install on the host):
  see [build_now.md](build_now.md).

## Documents

| Document | Content |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | Modules, data model, justified choices, Tauri IPC, protocol abstraction, verified versions |
| [SECURITY.md](SECURITY.md) | Threat model, key hierarchy, secret hygiene, Tauri hardening, accepted limitations |
| [EXPORT_FORMAT.md](EXPORT_FORMAT.md) | Versioned binary format of the vault and export/import bundles |
| [ROADMAP.md](ROADMAP.md) | Milestones M1 → M7, acceptance criteria, tests, CI |
| [MODE_D_EMPLOI.md](MODE_D_EMPLOI.md) | User and developer guide: prerequisites, hybrid Podman + host setup, commands, usage, v1 acceptance |
| [build_now.md](build_now.md) | Operational tutorial: build, test, install |
| [FONCTIONNEMENT_SSH.md](FONCTIONNEMENT_SSH.md) | How the SSH connection works end to end (resolution, host key, auth, shell channel, flows) |
| [LICENSE](LICENSE) | MIT license |

## Stack (fixed, versions verified on 2026-11-20)

- **Tauri 2.12** — **Rust** backend (tokio), **TypeScript / Svelte 5** frontend
- SSH/SFTP: **russh 0.65.2** + **russh-sftp 3.0.1** (pure Rust)
- Terminal: **native GTK+VTE** (libvte-2.91, direct FFI) — one window per session
- Editor: **CodeMirror 6**
- Crypto: **argon2 0.6** (Argon2id), **chacha20poly1305 0.11** (XChaCha20-Poly1305),
  **zeroize**, **secrecy**, **rand 0.10** (OsRng)
- OS keyring: **keyring 3.6.2** (Secret Service / KWallet) — **optional, never blocking**
- v1 target: **Linux** (.deb + .rpm); code is portable to Windows/macOS (not shipped)

## Non-negotiable principles

1. Secrets **never** reach the frontend, except on an explicit "show" / "copy" action
   (with automatic clipboard clearing).
2. The OS keyring is only a **local convenience cache**: it is never required for
   decryption. Without KWallet/Secret Service, the application works normally.
3. No home-made crypto: standard primitives, well-known crates, pinned versions.
4. Every milestone ships compiling and tested, with `cargo clippy -D warnings`,
   `cargo fmt`, `cargo audit`, ESLint + strict `tsc`.

## Transparency: AI-assisted development

This application was **created with the help of artificial intelligence: Claude (Anthropic, via
Claude Code) and Qwen (via Qwen Code)**. They wrote most of the code, tests and documentation.
Functional choices, trade-offs and validation (testing on a real workstation, bug reports) are
done by the human maintainer.

Things to know:
- the code is covered by automated tests and checked in CI (clippy, tests, audit), but it has
  **not undergone an independent security audit**; known limitations are described in
  [SECURITY.md](SECURITY.md) (§8);
- interface languages: French, English, Italian, German, Spanish and Portuguese;

## License

MIT — see [LICENSE](LICENSE).

The project is released under the **MIT license**: everyone is free to use, copy, modify,
redistribute and sell the application as they see fit, including commercially. It is provided
"as is", without warranty, with the only condition of keeping the copyright notice and the
license text.
