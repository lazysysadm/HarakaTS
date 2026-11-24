# BUILD_NOW — construire HarakaTS maintenant

Tutoriel opérationnel (2026-11-20). Principe : **tout se compile dans un conteneur
Podman** (Rust + Node, rien n'est installé sur l'OS), **l'app graphique tourne sur
l'hôte** Fedora avec le binaire/paquet produit. Aucune commande ci-dessous n'a
besoin de root, sauf l'installation des paquets hôte (faite par toi, une fois).

---

## 1. Prérequis hôte (une seule fois)

Paquets runtime pour lancer l'app + Podman pour builder :

```bash
sudo dnf install -y podman webkit2gtk4.1 gtk3 libsoup3 javascriptcoregtk4.1 glib2 vte291
```

- `webkit2gtk4.1 / gtk3 / libsoup3 / javascriptcoregtk4.1 / glib2` : moteur web de Tauri.
- `vte291` : moteur terminal natif (le même que Remmina).
- Déjà présents sur ton poste en principe (`rpm -q vte291 gtk3` pour vérifier).

## 2. Image de build (une seule fois, ou après modif du Containerfile)

L'image `local/harakats-dev` (Debian bookworm + Rust stable + Node 22 + libs GTK/WebKit/VTE
+ sshd de test) se construit depuis `packaging/Containerfile` :

```bash
podman build -t local/harakats-dev /opt/PROJECTS_AI/HarakaTS/packaging
```

Première fois : ~5–10 min (téléchargements). Les volumes nommés
`harakats-cargo-registry`, `harakats-cargo-git`, `harakats-pnpm-store` conservent
les caches entre builds → les builds suivants sont incrémentaux.

## 3. Build complet + vérification (la commande de référence)

Compile, formate, lint, teste (Rust dont SSH réel contre un sshd du conteneur +
UI), puis produit les paquets **.deb + .rpm**. C'est le bloc utilisé à chaque
jalon ; il doit finir sur `Finished 2 bundles` sans aucune erreur :

```bash
podman run --rm --name harakats-m3 -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 -v /opt/PROJECTS_AI/HarakaTS:/work:z -v harakats-cargo-registry:/root/.cargo/registry -v harakats-cargo-git:/root/.cargo/git -v harakats-pnpm-store:/root/.local/share/pnpm/store -w /work local/harakats-dev bash -c '
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

Pourquoi ces options :
- `-v …:/work:z` : **SELinux** (Fedora enforcing) — le `:z` autorise le conteneur
  à écrire dans le dossier projet. Sans lui : « Permission denied ».
- `COREPACK_ENABLE_DOWNLOAD_PROMPT=0` : pnpm via corepack sans question interactive.
- `xtask all` : icônes PNG (requises par le bundler) + fixtures de test du coffre.
- `xtask licenses` : génère `THIRD-PARTY-LICENSES.md` (cargo about + pnpm licenses),
  embarqué dans le binaire (À propos → Licences tierces) et bundlé dans les paquets.
- `cargo deny check licenses` : garde-fou — toute licence hors allowlist permissive
  (GPL/AGPL/inconnue) fait échouer le build (`deny.toml`).
- `sshd/setup.sh` + `HARAKA_SSHD_TEST=1` : serveur OpenSSH jetable dans le conteneur
  pour les tests d'interopérabilité réels (clés/config dans `crates/haraka-ssh/tests/sshd/run/`).
- `set -e` : s'arrête à la première erreur (pas de paquet produit si un test échoue).

Durée : premier build ~10–20 min ; incrémental ~2–6 min.

**Variante « build rapide »** (paquets sans la vérification — à n'utiliser qu'en
itération, jamais pour livrer) :

```bash
podman run --rm --name harakats-m3 -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 -v /opt/PROJECTS_AI/HarakaTS:/work:z -v harakats-cargo-registry:/root/.cargo/registry -v harakats-cargo-git:/root/.cargo/git -v harakats-pnpm-store:/root/.local/share/pnpm/store -w /work local/harakats-dev bash -c '
set -e
cargo run -p xtask -- all
pnpm install
pnpm --dir ui install
cargo run -p xtask -- licenses
pnpm --dir ui build
pnpm tauri build --bundles deb,rpm
'
```

## 4. Installer / réinstaller et lancer

Paquets produits (version courante **0.8.0** — source unique : `[workspace.package]
version` du `Cargo.toml` racine ; `cargo run -p xtask -- version check|sync|set X.Y.Z`
aligne `package.json` ×2 et `tauri.conf.json`, vérifié en CI) :
- `target/release/bundle/rpm/haraka-ts-0.8.0-1.x86_64.rpm` (Fedora)
- `target/release/bundle/deb/haraka-ts_0.8.0_amd64.deb` (Debian/Ubuntu)

Réinstallation Fedora + lancement avec logs (bloc unique) :

```bash
sudo dnf reinstall -y /opt/PROJECTS_AI/HarakaTS/target/release/bundle/rpm/haraka-ts-0.8.0-1.x86_64.rpm
RUST_LOG=harakats=debug,info harakats 2>/tmp/harakats.log
```

- `RUST_LOG` filtre les journaux du backend Rust (`tracing`) : `harakats=debug,info`
  = debug pour l'app, info pour le reste. Sans la variable : info seulement.
- Les logs partent dans `/tmp/harakats.log` (stderr) — c'est le fichier à envoyer
  en cas de diagnostic. Aucun secret n'y est écrit.
- Lancement sans logs : `harakats`. Binaire brut sans installation :
  `/opt/PROJECTS_AI/HarakaTS/target/release/harakats`.
- Profil de test vierge (coffre séparé) : `harakats --data-dir /tmp/harakats-profilB`.

## 5. Ce que « vert » veut dire

En fin de build complet, tu dois voir :
- `test result: ok.` sur toutes les suites Rust (0 failed) — dont
  `openssh_*` (interop réel) et le critère n°1 export A → import B ;
- `svelte-check found 0 errors and 0 warnings` ;
- `Tests  N passed` (vitest) ;
- `Finished 2 bundles at: … .deb … .rpm`.

Toute ligne `error[…]`, `FAILED` ou `ELIFECYCLE` = build non livrable : corriger
et relancer le bloc §3 (les caches rendent la reprise rapide).

## 6. Dépannage express

| Symptôme | Cause / solution |
|---|---|
| `Permission denied (os error 13)` sur `/work/…` | SELinux : le montage bind doit porter `:z` (déjà dans les blocs) |
| Corepack demande confirmation pour pnpm | `COREPACK_ENABLE_DOWNLOAD_PROMPT=0` (déjà dans les blocs) |
| `error while loading shared libraries: libwebkit2gtk-4.1.so.0` au lancement | libs runtime hôte manquantes → bloc §1 |
| Fenêtre terminal native absente / erreur VTE | `vte291` manquant → bloc §1 ; repli : Réglages → Moteur de terminal → xterm.js |
| `GLIBC_2.xx not found` sur un autre poste | binaire bookworm (glibc 2.36) : cible plus ancienne non supportée ; rebuild dans un conteneur aligné sur la cible |
| RPM refusé sur RHEL/Rocky/Alma 9 | EL9 non supporté (pas de webkit2gtk4.1) ; cibles : Fedora ≥ 42, EL10 via EPEL, Mageia |
| Tests interop ignorés (`HARAKA_SSHD_TEST != 1`) | normal hors conteneur ; le bloc §3 les active |
| `cargo tauri` introuvable | le CLI est installé via pnpm à la racine : utiliser `pnpm tauri …` |
| Build cassé après modif du Containerfile | reconstruire l'image (§2) puis relancer §3 |

## 7. Rappel des règles du projet

- Le build n'installe **rien sur l'OS hôte** (tout est dans le conteneur) ; seules
  les libs runtime §1 sont installées par toi.
- Git : commits/push **à la main du porteur du projet** (l'agent ne commite rien).
- Chaque jalon se livre avec ce build vert + résumé des écarts (ROADMAP.md) ;
  l'historique est dans MODE_D_EMPLOI.md §9.
- Docs de référence : `MODE_D_EMPLOI.md` (guide complet), `ARCHITECTURE.md`
  (versions épinglées, règles russh/VTE), `SECURITY.md`, `EXPORT_FORMAT.md`.