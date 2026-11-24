# ROADMAP — HarakaTS v1 (jalons M1 → M7)

> Phase 0 — plan de développement. À valider avant écriture de code.
> Règle : chaque jalon est **compilable, testé, livré avec son résumé** (fait / reste /
> écarts par rapport au plan). Pas de jalon « à moitié » : le critère d'acceptation
> principal (export A → import B) tourne en CI **dès M1** et reste vert ensuite.

## Définition of Done (chaque jalon)

- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo audit` (vulnérabilités bloquantes = 0), `cargo test --workspace` vert.
- `pnpm lint` (ESLint 9 + typescript-eslint), `pnpm check` (tsc strict), `pnpm test`
  (vitest) verts dès que l'UI existe (M2+).
- Tests du jalon exécutés et listés dans le résumé ; CI GitHub Actions à jour.
- MODE_D_EMPLOI.md mis à jour (nouvelles fonctionnalités utilisables).
- Aucun secret dans les logs/IPC/panics (revue ciblée à chaque jalon).
- Git : commits/push **à la main du porteur du projet** (je ne commite rien).

## Environnement (transverse, dès M1)

- Conteneur de dev Podman (`packaging/Containerfile`, Debian bookworm) : Rust stable
  (≥ 1.90), Node 22, pnpm, webkit2gtk-4.1-dev, GTK3, libsoup3, openssh-server (tests
  interop), dbus. L'OS hôte n'est pas modifié par le dev.
- Hôte : uniquement les libs **runtime** (installées par l'utilisateur, cf.
  MODE_D_EMPLOI.md) pour lancer le binaire produit.
- Serveur SSH de test double : in-process (russh server, hermétique) + openssh réel
  (conteneur dev / runner CI, port 2222, config et clés jetables dans `tests/`).

---

## M1 — Coffre, crypto, export/import (le cœur du projet)

**Périmètre** : `haraka-core` (vault, store, export, audit) + `src-tauri` minimal +
UI minimale (écran de création/déverrouillage/verrouillage, réglages coffre, dialogue
export/import). Pas encore d'arbre de connexions exploitable (M2).

- Coffre : DEK/KEK, Argon2id calibré (0,5–1 s), XChaCha20-Poly1305 double couche, AAD
  canoniques, écritures atomiques 0600/0700, backups rotatifs, restauration corruption.
- Changement de mot de passe maître (re-wrap seul), keyring cache opt-in **non bloquant**
  (KWallet/Secret Service via keyring 3.6.2 vendored ; absence → repli silencieux).
- Auto-lock : inactivité + `PrepareForSleep` + verrouillage session (zbus), zeroize.
- Export `.hrkx` (tout / dossier / sans secrets, passphrase distincte) et import
  (preview, conflits keep/replace/duplicate, re-chiffrement DEK locale, transactionnel).
- Journal d'audit local.
- **Tests normatifs EXPORT_FORMAT.md §9 complets**, dont :
  - round-trip A→B headless (deux data dirs, deux masters) ;
  - falsification bit-à-bit (header, nonce, payload, tag) → rejet ;
  - permutation de secrets (entrée/champ) → rejet AAD ;
  - fixtures versionnées.
- CI : workflow GitHub Actions (fmt/clippy/audit/test core + UI minimale).

**Acceptation** : créer un coffre A avec des credentials factices, exporter, importer
dans B vierge avec un autre master, retrouver tous les secrets à l'identique ; toute
altération du bundle est rejetée ; sans KWallet, tout fonctionne.

## M2 — Modèle de connexions, arbre, héritage, identifiants partagés

**Périmètre** : store complet (folders/connections/credentials/macros/recents/ui_state),
UI de gestion.

- Arbre dossiers/sous-dossiers : drag & drop, tags, couleurs, icônes, tri.
- Héritage des propriétés (fold defaults → dossiers → connexion), aperçu « valeur
  effective + provenance », surcharge explicite ; cycles interdits.
- Credentials découplés : CRUD, référencés par N connexions, `needs_reentry`,
  reveal/copy explicites avec effacement presse-papier N s, audit.
- Barre de connexion rapide `user@host:port`, favoris, récentes, recherche floue Ctrl+K
  (fuzzysort), i18n FR/EN.
- Réglages complets (politiques par défaut, auto-lock, KDF re-calibration, locale…).
- Écran de verrouillage/déverrouillage finalisé ; onglets détachables : préparation
  multi-fenêtres (capability).
- **Tests** : résolution d'héritage (matrice de cas), intégrité référentielle
  (suppression credential référencé), migration `schema_version`, CRUD store atomique,
  vitest composants arbre/héritage.

**Acceptation** : un dossier définit user/port/credential/jump/politique presse-papier ;
les connexions enfants héritent ou surchargent ; modifier un credential met à jour
toutes les connexions ; l'arbre se manipule entièrement au clavier et à la souris.

## M3 — Terminal SSH

**Périmètre** : `haraka-ssh` (session, auth, host keys, jump, keepalive/reconnect) +
UI terminal.

- Auth : mot de passe, clé (ed25519/ecdsa/rsa, chiffrées acceptées, passphrase coffre),
  keyboard-interactive 2 étapes (2FA/OTP, boucle de prompts), agent SSH (`SSH_AUTH_SOCK`).
- Host keys : TOFU + empreinte SHA256, known_hosts interne + import système (hashés),
  changement → alerte bloquante, audit.
- Jump hosts chaînés (`connect_stream` sur `direct-tcpip`), héritage des hops.
- Terminal xterm 6 : onglets, splits H/V, détachement en fenêtre, fit/search/web-links/
  unicode11/webgl (+ fallback DOM, contextloss), resize (`window_change`), flux
  `Channel<Vec<u8>>` batché, encodage configurable (encoding_rs).
- Keepalive (`send_keepalive`), latence (`send_ping`), reconnexion auto backoff,
  bannière serveur, prompts d'auth UI.
- **Tests** : serveur russh in-process (auth success/échec/partiel, keyboard-interactive
  multi-prompts, host key inconnue/changée, jump 2 hops, resize, keepalive) + interop
  openssh conteneur (auth réelle, PTY, shell). Reconnexion : coupure réseau simulée.

**Acceptation** : ouvrir une session vers le serveur de test avec chaque méthode
d'auth, à travers un jump host, avec TOFU puis alerte au changement de clé ; onglets,
splits et détachement fonctionnels ; aucune régression du test A→B.

## M4 — Presse-papier et politiques

**Périmètre** : politiques `disabled/l2r/r2l/bidirectional` héritables, OSC 52,
protections de collage, copy-on-select, clic milieu, fichiers entre panneaux.

- Handler OSC 52 (parser xterm) soumis à la politique + limite de taille (défaut 100 Ko) ;
  vérification du comportement OSC 52 natif de xterm 6 (le nôtre reste maître).
- Bracketed paste ; confirmation multiligne + détection contenu suspect (contrôle/
  échappements) avec aperçu exact ; copy-on-select optionnel ; sélection primaire Linux
  (arboard) ; Ctrl+Shift+C/V.
- Copier-coller de **fichiers** panneau local ↔ panneau distant (format interne) ;
  drag & drop OS → app (événements Tauri) → upload. Drag-out : limitation documentée.
- Effacement auto du presse-papier (secret copié) + avertissement historique Klipper.
- **Tests** : matrice des 4 politiques × 2 sens × héritage, OSC 52 surdimensionné →
  refus, collage suspect → confirmation, vitest logique de détection ; test manuel
  documenté (KDE/Klipper).

**Acceptation** : la politique d'un dossier s'applique aux connexions enfants ; OSC 52
respecte politique et taille ; un collage multiligne demande confirmation ; copier un
secret l'efface après N s.

## M5 — Navigateur SFTP et transferts  ✅ livré (2026-11-20)

> Écarts par rapport au plan : serveur russh in-process non utilisé pour les erreurs — tout
> passe par l'interop OpenSSH réel (permission refusée, chemin invalide réels) ; « disque
> plein » simulé non testé ; hash `check-file` et `chown` non livrés ; transferts non pipelinés ;
> permissions de dossiers non préservées. Voir MODE_D_EMPLOI §5.5 (limites v1).

**Périmètre** : panneau SFTP sur la même connexion (`into_stream` → `SftpSession`),
moteur de transferts complet.

- Arborescence + liste, cachés, tri, recherche ; renommer, supprimer, mkdir, chmod/chown,
  propriétés, copier chemin, symlinks.
- « Suivre le répertoire du terminal » : OSC 7 ; activation en un clic (ligne tapée dans le shell,
  opt-in, auditée) ou snippet à coller ; injection automatique à la connexion en option (réglage).
- Transferts : file, progression/débit/ETA (Channel), pause/reprise, **reprise après
  interruption** (`.part` + offset + seek), parallélisme (3 défaut, 8 max), récursif,
  permissions/dates préservées, intégrité (taille ; hash si extension `check-file` ;
  relecture complète opt-in), politique symlinks.
- **Tests** : interop openssh conteneur — gros fichier (100 Mi+) interrompu puis repris
  (octets identiques, hash local), transferts parallèles, dossier récursif avec
  permissions/dates, pause/reprise, serveur russh in-process pour les cas d'erreur
  (permission denied, disque plein simulé, chemin invalide).

**Acceptation** : un transfert interrompu reprend et produit un fichier identique ;
le panneau SFTP n'exige aucune seconde authentification ; opérations CRUD distantes
fonctionnelles.

## M6 — Édition à distance  ✅ livré (2026-11-20)

> Écarts : remplacement distant par sauvegarde `.bak` + rename (pas de `posix-rename` dans russh-sftp) ;
> retrait du moteur xterm (VTE unique) fait dans ce jalon. Voir ARCHITECTURE §11.

**Périmètre** : CodeMirror 6 intégré + éditeur externe, temp privé, watch, upload
automatique, conflits.

- Download → `data/tmp/<uuid>/` (0700/0600), coloration par extension, recherche,
  LF/CRLF, tabs ; éditeur externe configurable (argv direct, backend).
- Watcher `notify` + debounce ; upload atomique (temp distant + rename + metadata).
- Conflit : mtime/taille puis hash SHA-256 ; dialogue écraser/recharger/**comparer**
  (`@codemirror/merge`) ; purge des temps à la fermeture et au démarrage.
- **Tests** : scénario conflit (modification distante simulée entre download et save),
  upload atomique (pas de fichier partiel visible), nettoyage crash (purge démarrage),
  éditeur externe (commande factice), fins de ligne préservées.

**Acceptation** : éditer un fichier distant, sauvegarder → upload automatique ; si le
distant a changé entre-temps, le conflit est détecté et les 3 choix fonctionnent ;
aucun résidu temp après fermeture.

## M7 — Tunnels, multi-exec, imports, packaging (finalisation v1)  ✅ livré (2026-11-21)

> Écarts : `remmina.pref` (déchiffrement des mots de passe Remmina) non implémenté — `needs_reentry` ;
> `Match` / `ProxyCommand` d'ssh_config signalés non pris en charge ; clés embarquées MobaXterm non
> importées ; tunnels : pas de supervision d'un tunnel en erreur, bind non loopback averti mais non bloqué ;
> pas de poste vierge physique dans la CI (recette manuelle §5.13 MODE_D_EMPLOI).

**Périmètre** : tout le reste du prompt + livraison.

- Tunnels L/R/D : gestionnaire graphique (specs par connexion, live, auto_start),
  SOCKS5 minimal (CONNECT, loopback par défaut), remote forward via
  `server_channel_open_forwarded_tcpip`, avertissements bind/GatewayPorts.
- Multi-exec (sélection d'onglets, fan-out, indicateur permanent, confirmation).
- Macros/snippets (contenu chiffré, confirmation d'envoi).
- Journalisation de session opt-in (0600, avertissement écho).
- Imports : `~/.ssh/config` (Include, ProxyJump, IdentityFile → clés dans le coffre,
  hôtes joker → défauts de dossier), `.remmina` (INI, `needs_reentry`, best-effort
  expérimental avec remmina.pref fourni), `.mxtsessions` (INI-like, `needs_reentry`) ;
  rapports d'import, jamais de plantage.
- Packaging : **.deb + .rpm** (tauri bundler ; AppImage retirée, décision utilisateur
  du 2026-11-20), dépendances RPM vérifiées par `rpm -qp --requires`, cibles
  Fedora ≥ 42 / EL10 via EPEL / Mageia (**EL9 exclu**, cf. ARCHITECTURE §16), icônes,
  métadonnées `com.harakats.desktop`, mode portable (`portable.marker` / `--data-dir`).
- CI complète : fmt/clippy/audit/tests workspace + lint/tsc/vitest + interop openssh +
  **test A→B obligatoire** + build paquets.
- Documentation finale : MODE_D_EMPLOI.md complet, SECURITY.md relu, EXPORT_FORMAT.md
  figé v1, README à jour.
- **Tests** : tunnels L/R/D à travers le conteneur openssh (connexion réelle via
  tunnel, SOCKS5 curl), multi-exec (2 sessions, mêmes octets), imports sur fixtures
  réelles anonymisées (ssh config complexe, .remmina, .mxtsessions), paquets installés
  et lancés sur poste propre (checklist MODE_D_EMPLOI).

**Acceptation v1** : sur un poste Linux vierge (KDE ou sans keyring), installer le
.deb (Debian/Ubuntu) ou le .rpm (Fedora / EL10+EPEL), créer un coffre, importer une
config, ouvrir des sessions SSH avec tunnels et SFTP, éditer à distance, exporter,
réimporter ailleurs sans ressaisie. Tous les tests CI verts.

---

## Risques identifiés et mitigations

| Risque | Mitigation |
|---|---|
| API russh/russh-sftp mouvante (0.x) | versions épinglées + Cargo.lock commité (par toi) ; re-vérification docs.rs à chaque jalon ; serveur de test in-process pour découpler des aléas réseau |
| aead 0.6 / rand_core 0.10 : exemples anciens obsolètes | ARCHITECTURE §2.4 documente l'alignement ; tests crypto dès M1 |
| xterm 6 : viewport/scrollbar refondus, canvas supprimé | fallback DOM prévu ; tests visuels M3 ; épinglage strict des addons |
| OSC 52 natif xterm 6 en conflit avec notre politique | vérification M4, handler maître, option native désactivée si besoin |
| WebKit/GPU dans la webview (Wayland KDE) | fallback DOM renderer ; test sur poste réel dès M3 |
| russh : file interne du `Channel` bornée (100) → boucle SSH bloquée si non drainée (gel htop, constaté 2026-11-20) | canal `split()` + tâche de drain ; test `many_small_messages_do_not_stall_session` ; à conserver pour tout nouveau type de canal (SFTP M5, tunnels M6) |
| glibc hôte vs conteneur de build | build Debian bookworm (glibc 2.36) → compatible Debian 12+/Ubuntu 22.04+ et Fedora/EL10 (glibc plus récente) ; documenté |
| Cibles RPM sans webkit2gtk4.1 (EL9) | cibles v1 documentées : Fedora ≥ 42, EL10 via EPEL, Mageia ; **EL9 exclu** (tauri#9039) ; deps RPM vérifiées par `rpm -qp --requires` au M7 |
| Reprise de transfert SFTP (seek serveur) | test d'interruption réel dès M5 ; si le serveur ne supporte pas l'offset attendu → reprise depuis 0 avec avertissement |
| Dérive de périmètre | ce document + résumé d'écarts à chaque fin de jalon ; toute fonctionnalité non listée = reportée post-v1 par défaut |

## Ordre et dépendances

```
M1 (coffre) ──▶ M2 (modèle/UI) ──▶ M3 (SSH) ──▶ M4 (presse-papier)
                                   │
                                   ├──▶ M5 (SFTP) ──▶ M6 (édition)
                                   └──▶ M7 (tunnels/multi-exec/imports/packaging)
```

M4 ne dépend que de M3 ; M5/M6 dépendent de M3 ; M7 clôture. Le test A→B (M1) est
rejoué à chaque jalon dans la CI.

## Après la v1 (versions 0.N)

| Version | Contenu | État |
|---|---|---|
| 0.7.0 / 0.7.1 | M7 + durcissement sécurité (clé d'hôte bloquante, intégrité du coffre, journaux chiffrés, tmpfs, agent/FIDO2), versionnement, créateur | livré |
| 0.8.0 | Barre de sessions, SFTP lié à la session active, terminaux en onglets (option), multi-exec flottant + macros, coloration de la sortie | livré |
| 0.9.0 | SFTP seul (sans terminal), « Tester la connexion », authentification non interactive puis interactive | prévu (`feat/sftp-only`) |
| 1.0.0 | Recette §5.13 validée en entier | à venir |
