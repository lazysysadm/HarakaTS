# Contributing to HarakaTS

Thanks for your interest. HarakaTS is a Tauri 2 desktop SSH/SFTP manager (Rust + Svelte 5).

## Build and test

The reference build runs in a Podman container, see [MODE_D_EMPLOI.en.md](MODE_D_EMPLOI.en.md) §4 and
[build_now.en.md](build_now.en.md). With a local toolchain (Rust stable, Node 22, pnpm):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm install && pnpm --dir ui install
pnpm --dir ui lint && pnpm --dir ui check && pnpm --dir ui test
```

## Rules

- Branches: `feat/*` and `fix/*` merge into `dev`; `main` holds tagged releases.
- Every user-visible string goes through i18n and exists in the 6 locales (fr, en, de, es, it, pt). A test enforces this.
- Dependencies must pass `cargo deny check licenses` (permissive licenses only, no GPL/AGPL).
- Never hold the global session lock across an `.await` on the network.
- Secrets never go to logs, errors or the UI.

## Security

Report vulnerabilities privately, see [SECURITY.en.md](SECURITY.en.md).
