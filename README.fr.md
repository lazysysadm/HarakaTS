# HarakaTS

[![CI](https://github.com/lazysysadm/HarakaTS/actions/workflows/ci.yml/badge.svg)](https://github.com/lazysysadm/HarakaTS/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/lazysysadm/HarakaTS)](https://github.com/lazysysadm/HarakaTS/releases/latest)
[![License: MIT](https://img.shields.io/github/license/lazysysadm/HarakaTS)](LICENSE)
![Platform: Linux](https://img.shields.io/badge/platform-Linux-informational)
![Rust](https://img.shields.io/badge/Rust-stable-orange?logo=rust)
![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri)
![Svelte 5](https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte)

🇬🇧 [English version](README.md) · 🇫🇷 Français

📚 Documentation anglaise : [Sécurité](SECURITY.en.md) · [Architecture](ARCHITECTURE.en.md) · [Guide utilisateur](MODE_D_EMPLOI.en.md) · [Format d'export](EXPORT_FORMAT.en.md) · [Feuille de route](ROADMAP.en.md) · [Builder maintenant](build_now.en.md) · [Fonctionnement SSH](FONCTIONNEMENT_SSH.en.md)

**Créé par LazySysAdmin.**

Gestionnaire de connexions distantes desktop pour Linux (SSH + SFTP en v1), au niveau de
Remmina / MobaXterm, avec une exigence centrale : **des secrets chiffrés et
portables**. Un export réimporté sur un autre poste retrouve tous les mots de passe et
toutes les clés, sans aucune ressaisie — contrairement à Remmina dont les secrets sont
liés au keyring local.

## État du projet

**v1 livrée** (jalons M1 → M7, 2026-11-21). Coffre chiffré portable et export/import ; arbre de
connexions héritable ; terminal natif GTK+VTE ; SFTP (navigateur deux panneaux, transferts
reprenables) ; édition à distance (CodeMirror 6 / éditeur externe, détection de conflit) ; tunnels
L/R/D ; barre de sessions, terminaux en onglets façon mRemoteNG (option) avec panneau SFTP à la demande,
pastilles d'état des serveurs, verrouillage sans déconnexion ; multi-exec (panneau flottant), macros, journalisation de session, coloration de la sortie ; imports `~/.ssh/config`, Remmina et MobaXterm ;
interface en 6 langues (fr, en, it, de, es, pt) ; paquets `.deb` et `.rpm`.
Le code se construit et se teste dans le conteneur Podman de dev
(voir [MODE_D_EMPLOI.md](MODE_D_EMPLOI.md) §4 ; tutoriel pas à pas : [build_now.md](build_now.md)) ;
la checklist de recette v1 est au §5.13 du guide.

## Installation

HarakaTS cible **Linux** (bureau avec GTK 3). Les paquets précompilés sont joints à chaque release
(`.rpm` pour Fedora / openSUSE / famille RHEL, `.deb` pour Debian / Ubuntu et dérivées). Le gestionnaire
de paquets installe seul les dépendances d'exécution (WebKitGTK 4.1, GTK 3, libsoup 3, VTE 2.91).

**Fedora / famille RHEL**

```bash
sudo dnf install ./haraka-ts-<version>-1.x86_64.rpm
```

**Debian / Ubuntu et dérivées**

```bash
sudo apt install ./haraka-ts_<version>_amd64.deb
```

Lance ensuite **HarakaTS** depuis le menu des applications, ou `harakats` dans un terminal.
`harakats --version` affiche la version et le commit de construction.

- **Premier démarrage :** choisis une passphrase maître pour créer le coffre chiffré. Elle n'est pas
  récupérable : conserve-la en lieu sûr. Le keyring du système est optionnel et jamais requis.
- **Tes données** restent dans `~/.local/share/harakats/` (coffre, sauvegardes rotatives, known_hosts
  interne, journal d'audit). *Mode portable* : place un fichier `portable.marker` à côté du binaire, ou
  passe `--data-dir <chemin>`, pour tout garder dans un dossier `./data/` (clé USB, dossier synchronisé).
- **Mise à jour :** installe le paquet plus récent par-dessus l'ancien (`dnf upgrade ./…rpm` /
  `apt install ./…deb`) ; tes données ne sont pas touchées. Si le numéro de version est identique, utilise
  `sudo dnf reinstall ./…rpm`.
- **Désinstallation :** `sudo dnf remove haraka-ts` ou `sudo apt remove haraka-ts`. Ton coffre reste dans
  `~/.local/share/harakats/` tant que tu ne le supprimes pas toi-même.
- **Passage depuis une ancienne release** nommée `HarakaTS` (avant le renommage en `haraka-ts`) : le nouveau RPM déclare `Obsoletes: HarakaTS`, donc `sudo dnf install ./haraka-ts-<version>-1.x86_64.rpm` la remplace ; tes données ne sont pas touchées.
- **Vérifier un téléchargement :** compare `sha256sum <fichier>` avec la somme publiée avec la release.
- **Construire depuis les sources** (Rust + Node, dans un conteneur Podman, rien à installer sur l'hôte) :
  voir [build_now.md](build_now.md).

## Documents

| Document | Contenu |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | Modules, modèle de données, choix justifiés, IPC Tauri, abstraction protocoles, versions vérifiées |
| [SECURITY.md](SECURITY.md) | Modèle de menace, hiérarchie de clés, hygiène des secrets, durcissement Tauri, limites assumées |
| [EXPORT_FORMAT.md](EXPORT_FORMAT.md) | Format binaire versionné du coffre et des bundles d'export/import |
| [ROADMAP.md](ROADMAP.md) | Jalons M1 → M7, critères d'acceptation, tests, CI |
| [MODE_D_EMPLOI.md](MODE_D_EMPLOI.md) | Guide utilisateur et développeur : prérequis, env hybride Podman + hôte, commandes, utilisation, recette v1 |
| [build_now.md](build_now.md) | Tutoriel opérationnel : construire, tester, installer |
| [FONCTIONNEMENT_SSH.md](FONCTIONNEMENT_SSH.md) | Fonctionnement de la connexion SSH de bout en bout (résolution, clé d'hôte, auth, canal shell, flux) |
| [LICENSE](LICENSE) | Licence MIT |

## Stack (imposée, versions vérifiées le 2026-11-20)

- **Tauri 2.12** — backend **Rust** (tokio), frontend **TypeScript / Svelte 5**
- SSH/SFTP : **russh 0.65.2** + **russh-sftp 3.0.1** (pur Rust)
- Terminal : **GTK+VTE natif** (libvte-2.91, FFI direct) — une fenêtre par session
- Éditeur : **CodeMirror 6**
- Crypto : **argon2 0.6** (Argon2id), **chacha20poly1305 0.11** (XChaCha20-Poly1305),
  **zeroize**, **secrecy**, **rand 0.10** (OsRng)
- Keyring OS : **keyring 3.6.2** (Secret Service / KWallet) — **optionnel, jamais bloquant**
- Cible v1 : **Linux** (.deb + .rpm), code portable Windows/macOS (non livrés)

## Principes non négociables

1. Les secrets ne transitent **jamais** vers le frontend, sauf action explicite
   « afficher » / « copier » (avec effacement automatique du presse-papier).
2. Le keyring OS n'est qu'un **cache de confort local** : il n'est jamais indispensable
   au déchiffrement. Sans KWallet/Secret Service, l'application fonctionne normalement.
3. Pas de crypto maison : primitives standard, crates reconnues, versions épinglées.
4. Chaque jalon est livré compilable, testé, avec `cargo clippy -D warnings`,
   `cargo fmt`, `cargo audit`, ESLint + `tsc` strict.

## Transparence : développement assisté par IA

Cette application a été **créée à l'aide d'intelligences artificielles : Claude (Anthropic, via
Claude Code) et Qwen (via Qwen Code)**. Elles ont écrit l'essentiel du code, des tests et de la
documentation. Les choix fonctionnels, les arbitrages et la validation (tests sur poste réel,
retours de bugs) sont faits par le mainteneur humain.

Conséquences à connaître :
- le code est couvert par des tests automatisés et vérifié en CI (clippy, tests, audit), mais il n'a
  **pas fait l'objet d'un audit de sécurité indépendant** ; les limites connues sont décrites dans
  [SECURITY.md](SECURITY.md) (§8) ;
- langues de l'interface : français, anglais, italien, allemand, espagnol et portugais ;

## Licence

MIT — voir [LICENSE](LICENSE).

Le projet est sous **licence MIT** : chacun est libre d'utiliser, copier, modifier, redistribuer et
vendre l'application comme il l'entend, y compris à titre commercial. Elle est fournie « telle
quelle », sans garantie, avec pour seule condition de conserver la mention de copyright et le texte
de la licence.
