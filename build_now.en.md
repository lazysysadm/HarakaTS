# BUILD_NOW — build HarakaTS now

Operational tutorial (2026-11-20). Principle: **everything compiles inside a Podman
container** (Rust + Node, nothing is installed on the OS), **the GUI app runs on the Fedora
host** with the produced binary/package. No command below needs root, except the host
package installation (done by you, once). Run every command from the project root (the folder that contains `Cargo.toml`).

---

## 1. Host prerequisites (once)

Runtime packages to run the app + Podman to build:

```bash
sudo dnf install -y podman webkit2gtk4.1 gtk3 libsoup3 javascriptcoregtk4.1 glib2 vte291
```

- `webkit2gtk4.1 / gtk3 / libsoup3 / javascriptcoregtk4.1 / glib2`: Tauri's web engine.
- `vte291`: native terminal engine (the same as Remmina).
- Already present on your machine in principle (`rpm -q vte291 gtk3` to check).

## 2. Build image (once, or after a Containerfile change)

The `local/harakats-dev` image (Debian bookworm + Rust stable + Node 22 + GTK/WebKit/VTE
libs + test sshd) is built from `packaging/Containerfile`:

```bash
podman build -t local/harakats-dev ./packaging
```

First time: ~5–10 min (downloads). The named volumes `harakats-cargo-registry`,
`harakats-cargo-git`, `harakats-pnpm-store` keep the caches between builds → subsequent
builds are incremental.

## 3. Full build + verification (the reference command)

Compiles, formats, lints, tests (Rust including real SSH against a container sshd + UI),
then produces the **.deb + .rpm** packages. This is the block used at every milestone; it
must end with `Finished 2 bundles` without any error:

```bash
podman run --rm --name harakats-m3 -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 -v "$PWD":/work:z -v harakats-cargo-registry:/root/.cargo/registry -v harakats-cargo-git:/root/.cargo/git -v harakats-pnpm-store:/root/.local/share/pnpm/store -w /work local/harakats-dev bash -c '
set -e
bash crates/haraka-ssh/tests/sshd/setup.sh
export HARAKA_SSHD_TEST=1 HARAKA_TEST_KEY=/work/crates/haraka-ssh/tests/sshd/run/client_ed25519 HARAKA_TEST_USER=harakats-test HARAKA_TEST_PORT=2222
cargo run -p xtask -- all
pnpm install
pnpm --dir ui install
cargo run -p xtask -- licenses
pnpm --dir ui build
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check licenses
cargo test --workspace
pnpm --dir ui lint
pnpm --dir ui check
pnpm --dir ui test
pnpm tauri build --bundles deb,rpm
'
```

Why these options:
- `-v …:/work:z`: **SELinux** (Fedora enforcing) — the `:z` allows the container to write
  into the project folder. Without it: "Permission denied".
- `COREPACK_ENABLE_DOWNLOAD_PROMPT=0`: pnpm via corepack without an interactive question.
- `xtask all`: PNG icons (required by the bundler) + vault test fixtures.
- `xtask licenses`: generates `THIRD-PARTY-LICENSES.md` (cargo about + pnpm licenses),
  embedded in the binary (About → Third-party licenses) and bundled in the packages.
- `cargo deny check licenses`: guardrail — any license outside the permissive allowlist
  (GPL/AGPL/unknown) fails the build (`deny.toml`).
- `sshd/setup.sh` + `HARAKA_SSHD_TEST=1`: disposable OpenSSH server in the container for
  real interoperability tests (keys/config in `crates/haraka-ssh/tests/sshd/run/`).
- `set -e`: stops at the first error (no package produced if a test fails).

Duration: first build ~10–20 min; incremental ~2–6 min.

**"Quick build" variant** (packages without the verification — to use only while iterating,
never to deliver):

```bash
podman run --rm --name harakats-m3 -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 -v "$PWD":/work:z -v harakats-cargo-registry:/root/.cargo/registry -v harakats-cargo-git:/root/.cargo/git -v harakats-pnpm-store:/root/.local/share/pnpm/store -w /work local/harakats-dev bash -c '
set -e
cargo run -p xtask -- all
pnpm install
pnpm --dir ui install
cargo run -p xtask -- licenses
pnpm --dir ui build
pnpm tauri build --bundles deb,rpm
'
```

## 4. Install / reinstall and launch

Produced packages (current version **0.8.0** — single source: `[workspace.package] version`
of the root `Cargo.toml`; `cargo run -p xtask -- version check|sync|set X.Y.Z` aligns
`package.json` ×2 and `tauri.conf.json`, checked in CI):
- `target/release/bundle/rpm/haraka-ts-0.8.0-1.x86_64.rpm` (Fedora)
- `target/release/bundle/deb/haraka-ts_0.8.0_amd64.deb` (Debian/Ubuntu)

Fedora reinstall + launch with logs (single block):

```bash
sudo dnf reinstall -y target/release/bundle/rpm/haraka-ts-0.8.0-1.x86_64.rpm
RUST_LOG=harakats=debug,info harakats 2>/tmp/harakats.log
```

- `RUST_LOG` filters the Rust backend logs (`tracing`): `harakats=debug,info` = debug for
  the app, info for the rest. Without the variable: info only.
- Logs go to `/tmp/harakats.log` (stderr) — this is the file to send for diagnostics. No
  secret is written there.
- Launch without logs: `harakats`. Raw binary without installation:
  `target/release/harakats`.
- Virgin test profile (separate vault): `harakats --data-dir /tmp/harakats-profilB`.

## 5. What "green" means

At the end of the full build, you must see:
- `test result: ok.` on every Rust suite (0 failed) — including `openssh_*` (real interop)
  and criterion #1 export A → import B;
- `svelte-check found 0 errors and 0 warnings`;
- `Tests  N passed` (vitest);
- `Finished 2 bundles at: … .deb … .rpm`.

Any `error[…]`, `FAILED` or `ELIFECYCLE` line = non-deliverable build: fix and rerun the §3
block (the caches make the restart fast).

## 6. Quick troubleshooting

| Symptom | Cause / solution |
|---|---|
| `Permission denied (os error 13)` on `/work/…` | SELinux: the bind mount must carry `:z` (already in the blocks) |
| Corepack asks confirmation for pnpm | `COREPACK_ENABLE_DOWNLOAD_PROMPT=0` (already in the blocks) |
| `error while loading shared libraries: libwebkit2gtk-4.1.so.0` at launch | missing host runtime libs → §1 block |
| Native terminal window absent / VTE error | missing `vte291` → §1 block; fallback: Settings → Terminal engine → xterm.js |
| `GLIBC_2.xx not found` on another machine | bookworm binary (glibc 2.36): older target not supported; rebuild in a container aligned with the target |
| RPM refused on RHEL/Rocky/Alma 9 | EL9 not supported (no webkit2gtk4.1); targets: Fedora ≥ 42, EL10 via EPEL, Mageia |
| Interop tests ignored (`HARAKA_SSHD_TEST != 1`) | normal outside the container; the §3 block enables them |
| `cargo tauri` not found | the CLI is installed via pnpm at the root: use `pnpm tauri …` |
| Build broken after a Containerfile change | rebuild the image (§2) then rerun §3 |

## 7. Project rules reminder

- The build installs **nothing on the host OS** (everything is in the container); only the
  §1 runtime libs are installed by you.
- Git: commits/push **in the hands of the project owner** (the agent commits nothing).
- Every milestone is delivered with this green build + deviation summary (ROADMAP.en.md);
  the history is in MODE_D_EMPLOI.en.md §9.
- Reference docs: `MODE_D_EMPLOI.en.md` (complete guide), `ARCHITECTURE.en.md` (pinned
  versions, russh/VTE rules), `SECURITY.en.md`, `EXPORT_FORMAT.en.md`.