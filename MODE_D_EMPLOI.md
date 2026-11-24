# MODE D'EMPLOI — HarakaTS

> Guide utilisateur et développeur. Mis à jour à chaque jalon.
> **État actuel : M1 livré (coffre + crypto + export/import + UI minimale)** —
> docs Phase 0 validés le 2026-11-20. Build et tests via le conteneur Podman (§4) ;
> les sections « Utilisation » s'activent au fil des jalons M2 → M7 (voir ROADMAP.md).

## 1. Ce qu'est HarakaTS

Un gestionnaire de connexions SSH/SFTP pour Linux (terminaux à onglets, panneau SFTP,
édition distante, tunnels, multi-exec), avec un coffre chiffré **portable** : un export
réimporté sur un autre poste retrouve tous les mots de passe et toutes les clés, sans
ressaisie. Le keyring OS (KWallet) n'est qu'un confort optionnel : **sans lui, tout
fonctionne**, avec le mot de passe maître.

## 2. Architecture de l'environnement de dev (hybride Podman + hôte)

Choix validé : **compiler et tester dans un conteneur Podman** (l'OS du poste reste
propre), **lancer l'app graphique sur l'hôte** avec le binaire produit.

```
┌─────────────────────────────┐        ┌──────────────────────────────┐
│ Conteneur Podman (dev)      │        │ Hôte (KDE)                   │
│  Rust ≥ 1.90, Node 22, pnpm │ build  │  libs runtime webkit2gtk-4.1 │
│  webkit2gtk-4.1-dev, GTK3   │ ─────▶ │  ./target/release/harakats   │
│  openssh-server (tests)     │        │  données ~/.local/share/…    │
│  cargo test / clippy / audit│        │  (GUI, keyring optionnel)    │
└─────────────────────────────┘        └──────────────────────────────┘
```

- Le code source est monté dans le conteneur (`/work`) ; tu édites avec tes outils
  habituels sur l'hôte.
- Le conteneur est construit depuis `packaging/Containerfile` (créé au M1, contenu
  ci-dessous) ; image `local/harakats-dev`.
- Les tests SSH/SFTP tournent **dans** le conteneur (sshd local port 2222 + serveur
  russh in-process) : pas de conteneur imbriqué, pas de service sur l'hôte.
- Limite assumée : pas de hot-reload `tauri dev` sur l'hôte sans toolchain hôte ; la
  boucle de dev est « éditer → rebuild conteneur → relancer le binaire ». Si tu veux un
  jour le hot-reload, on avisera pour installer rustup/fnm sur l'hôte (ton choix).

## 3. Prérequis à installer PAR TOI (je n'ai pas root, je n'installe rien)

Le poste de dev est **Fedora 45 KDE** (SELinux enforcing : les montages Podman du
projet doivent porter `:z`). Sur l'hôte, uniquement les bibliothèques **runtime**
pour lancer le binaire produit (Podman est déjà en place) :

```bash
sudo dnf install -y webkit2gtk4.1 gtk3 libsoup3 javascriptcoregtk4.1 glib2
podman --version
```

Sur une cible Debian 12+ / Ubuntu 22.04+ (autre poste), l'équivalent apt :

```bash
sudo apt-get update
sudo apt-get install -y libwebkit2gtk-4.1-0 libgtk-3-0 libsoup-3.0-0 libjavascriptcoregtk-4.1-0 libglib2.0-0
```

Notes :
- Les paquets produits déclarent leurs dépendances runtime :
  `apt install ./harakats_*.deb` (Debian/Ubuntu) ou `dnf install ./harakats-*.rpm`
  (Fedora ; RHEL/Rocky/Alma **10** avec EPEL activé) tirent tout automatiquement.
- **Cibles RPM** : Fedora ≥ 42, EL10 via EPEL, Mageia 9/10. **EL9 (RHEL/Rocky/Alma 9)
  non supporté** : webkit2gtk4.1 y est absent et glib2 y est trop ancien pour la pile
  soup3 de Tauri v2.
- **KWallet (KDE)** : optionnel. Pour activer le cache de déverrouillage, KWallet doit
  exposer l'interface Secret Service (réglages du portefeuille KDE). Sans cela,
  HarakaTS démarre et fonctionne normalement, mot de passe maître à chaque
  déverrouillage — aucun message bloquant.
- Aucun paquet n'est installé sur l'hôte pour le développement : tout est dans le
  conteneur.

## 4. Conteneur de dev (à partir de M1)

> **Tuto opérationnel pas à pas : [`build_now.md`](build_now.md)** (prérequis, image,
> build complet/rapide, réinstallation, dépannage — blocs copiables uniques).

`packaging/Containerfile` (contenu prévu, sera créé au M1) :

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

Debian bookworm (glibc 2.36) est choisi pour que le binaire produit tourne sur
Debian 12+ / Ubuntu 22.04+ (compatibilité ascendante glibc).

Construction et entrée dans le conteneur (bloc unique, à lancer depuis la racine du
projet) :

```bash
podman build -t local/harakats-dev -f packaging/Containerfile packaging/
podman run --rm -it --name harakats-dev \
  -v "$PWD":/work:z \
  -v harakats-cargo-registry:/root/.cargo/registry \
  -v harakats-cargo-git:/root/.cargo/git \
  -v harakats-pnpm-store:/root/.local/share/pnpm/store \
  -w /work local/harakats-dev bash
```

Dans le conteneur, boucle de développement M1 (bloc unique — l'UI est buildée
**avant** clippy/tests car `generate_context!` embarque `ui/dist` à la compilation) :

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

`xtask all` génère les icônes PNG (requises par le bundler) et les fixtures versionnées
du format v1 (`tests/fixtures/*.hrk*`, passphrases publiques de test). Le CLI Tauri est
installé à la racine (`pnpm tauri`, pas `cargo tauri`). Premier build : téléchargement
des crates et compilation (~10–20 min) ; les suivants sont incrémentaux.

Sur l'hôte, lancer l'app produite (bloc unique) :

```bash
target/release/harakats
```

(En dev itératif : `cargo build` puis `target/debug/harakats`. Les paquets produits :
`target/release/bundle/deb/*.deb`, `target/release/bundle/rpm/*.rpm`.)

### 4.1 Variables d'environnement utiles (diagnostic)

| Variable | Effet |
|---|---|
| `RUST_LOG=harakats=debug,info` | logs backend détaillés (ouverture/fermeture sessions, resize, lots de sortie, erreurs). Capturer : `RUST_LOG=harakats=debug,info harakats 2>/tmp/harakats.log` |
| `GDK_BACKEND=x11` | force X11 au lieu de Wayland (isoler un problème Wayland) |
| `WEBKIT_DISABLE_COMPOSITING_MODE=1` | désactive le compositing de la webview Tauri (isoler un problème GPU/webview ; sans effet sur le terminal VTE natif) |

## 5. Utilisation de l'application

### 5.1 Premier démarrage (M1 — disponible)

1. Lancer HarakaTS → écran de création du coffre.
2. Choisir un **mot de passe maître** robuste (phrase de passe longue recommandée).
   Il chiffre tout ; **oublié = données perdues** (aucune porte dérobée).
3. Les paramètres Argon2id sont calibrés automatiquement (~0,5–1 s sur ta machine).
4. Optionnel : activer le cache KWallet (réglages → coffre). Sans KWallet, rien ne
   change à part la saisie du mot de passe maître à chaque déverrouillage.

### 5.2 Connexions et identifiants (M2 — disponible)

- Arbre de dossiers (glisser-déposer avec anti-cycle, couleurs, tags, icônes,
  renommage inline, navigation clavier ↑/↓/Entrée) ; un dossier peut définir des
  valeurs par défaut (utilisateur, port, identifiant, jump host, politique
  presse-papier, keepalive, encodage) héritées par ses enfants.
- L'éditeur de connexion affiche la **configuration effective avec provenance**
  (« défaut système », « hérité de <dossier> », « surcharge connexion ») ; chaque
  champ a une case « hériter » pour remettre la valeur par défaut.
- Les **identifiants sont découplés** : crée un credential (login + mot de passe et/ou
  clé + passphrase), référence-le depuis N connexions ; une modification suffit partout.
  Suppression : refusée si référencé, ou détachement explicite des références.
- **Afficher / Copier** un secret : actions explicites journalisées (audit) ; la copie
  passe par le backend et s'efface du presse-papier après N secondes (réglable) ;
  l'affichage à l'écran se masque automatiquement après 30 s.
- **Importer une clé privée depuis un fichier** (Identifiants → « Importer un
  fichier… », dossier initial `~/.ssh`) : lecture côté backend uniquement — la clé
  ne passe ni par le presse-papier ni par l'interface ; seules les métadonnées
  s'affichent (type, empreinte SHA256, chiffrée ou non). Clé chiffrée : la
  passphrase est **vérifiée avant enregistrement**. Clé publique (.pub) refusée
  avec explication ; avertissement si les permissions du fichier source sont trop
  larges ; la clé publique voisine `<fichier>.pub` est affichée si présente.
  Import audité (nom du fichier + empreinte, jamais le contenu).
- **Ctrl+K** : recherche floue (connexions, dossiers, identifiants, actions) ; favoris dans la barre
  latérale. La barre de connexion rapide a été retirée (elle ne servait pas sans identifiants existants) ;
  une connexion se crée depuis l'arbre (bouton de groupe ou clic droit).
  La barre d'outils ne contient que **☰ Menu** (SFTP, éditeur, tunnels, multi-exec, macros, identifiants,
  import/export, audit, réglages, palette, À propos) et **🔒**.
- Interface **FR/EN** (réglages → langue). Export par dossier disponible (M2).

### 5.3 Terminal SSH (M3 / M3-VTE — disponible)

- **Moteur par défaut : VTE natif (GTK) — le moteur de Remmina.** Chaque connexion
  (▶, double-clic, Ctrl+K) ouvre une **fenêtre terminal native**
  1100×700 : rendu, clavier, resize et copier/coller (Ctrl+Shift+C/V natifs) gérés
  par le système, hors webview — htop/tmux/vim ne peuvent plus figer l'affichage.
  Le PTY serveur est ouvert aux dimensions réelles de la fenêtre ; le resize est
  répercuté à la volée (debounce 100 ms). Prompts auth et clés d'hôte = dialogues
  natifs. Fermer la fenêtre ferme la session ; verrouiller le coffre ferme tout.
- **Barre d'état native** sous le terminal : état/latence, dimensions cols×rows,
  bouton **Reconnecter**. Les erreurs de connexion s'affichent en rouge DANS le
  terminal. **Menu contextuel** (clic droit) : Copier, Coller, Tout sélectionner,
  Effacer l'écran, Réinitialiser le terminal.
- **Mode fenêtres (défaut)** : une barre de sessions sous la barre d'outils (une pastille par session,
  point d'état vert/jaune/rouge ; un clic ramène le terminal, ✕ ferme la session) et le panneau SFTP
  qui s'ouvre sur la session active.
- **Terminaux en onglets** — Réglages → « Terminaux en onglets dans la fenêtre principale »
  (appliqué aux sessions ouvertes ensuite) : disposition façon mRemoteNG. À gauche, la **colonne des
  connexions** (arbre, menu) : étroite (240 px par défaut) et **réglable à la souris entre 200 et
  420 px** ; à droite, les **terminaux en onglets** (point d'état coloré, ✕ pour fermer ; au moins 360 px).
  Quand une vue large s'ouvre (SFTP, éditeur, tunnels, multi-exec, fenêtres de réglages…), la colonne passe
  automatiquement à une largeur **fixe** (~55 %, 880 px minimum) puis revient à la largeur choisie à
  la fermeture. Changer d'onglet change la session active.
- **SFTP à la demande** : le bouton **Fichiers (SFTP)** de la barre d'état du terminal (à côté de
  Reconnecter) ouvre le panneau SFTP sur la session de ce terminal. « Suivre le terminal » mémorise le
  dernier dossier de chaque shell (OSC 7) : il survit au changement d'onglet. En mode onglets, rien
  ne s'ouvre tout seul.
- **État des serveurs** (option, désactivée par défaut) — Réglages → « Afficher l'état des serveurs » :
  pastille verte/rouge/grise devant chaque connexion de l'arbre. Sonde = simple **connexion TCP** vers
  le port SSH, refermée aussitôt (pas d'ICMP, aucun échange SSH) ; intervalle réglable (60 s par défaut,
  15 s minimum), un test par hôte:port, 8 en parallèle au plus, délai de 2 s, rien si le coffre est
  verrouillé ; les connexions derrière un rebond SSH ne sont pas testées (grise).
- **Coloration de la sortie** — Réglages → « Colorer la sortie du terminal » (activé par défaut) :
  règles fixes, rouge (error, failed, fatal, critical, denied, refused…), jaune (warning,
  deprecated, timeout), vert (success, ok, passed, enabled, `active (running)`), cyan (IPv4, URL).
  Dates, heures et chemins ne sont pas colorés. Jamais appliquée dans un programme plein écran
  (vim, htop, less, tmux), ni par-dessus une couleur posée par le serveur (`ls --color`,
  `grep --color`). Les journaux de session restent bruts. Appliqué aux sessions ouvertes ensuite.
- Saisie et resize sont traités **dans l'ordre** par une tâche unique par session ;
  sortie coalescée (un seul envoi GTK en vol) ; police et scrollback configurables
  dans les Réglages (appliqués à l'ouverture des fenêtres).
- **Moteur unique : VTE natif** (plus de choix de moteur dans les Réglages ; le repli xterm.js web, ses
  onglets, splits et détachements ont été retirés en M6).
- Auth : mot de passe, clé (ed25519/ecdsa/rsa, chiffrées acceptées), 2FA/OTP
  (keyboard-interactive, prompts à l'écran), agent SSH local — dans l'ordre
  défini par l'héritage ; les matériaux sont résolus par le backend.
- Première connexion à un hôte : empreinte SHA256 affichée, accepter/refuser (TOFU).
  Si la clé change ensuite : **alerte bloquante** — ne pas accepter à l'aveugle.
  Réglages → « Clés d'hôtes » : liste des empreintes + import de `~/.ssh/known_hosts`.
- Jump hosts : chaîne de bastions configurable par connexion (héritable).
- Keepalive + latence affichée, **reconnexion automatique** (backoff), encodage
  terminal configurable. Moteur web : recherche, liens cliquables, Unicode 11,
  Ctrl+Shift+C/V via presse-papier backend.
- Au verrouillage du coffre : toutes les sessions sont fermées.

### 5.4 Presse-papier (M4 — disponible)

- **Politique héritable** par groupe/connexion : `disabled` / `l2r` (local→distant) /
  `r2l` (distant→local) / `bidirectional` (défaut). Réglages → politique par défaut ;
  l'éditeur de connexion affiche la valeur effective + provenance.
- **OSC 52 intercepté dans le backend** (les séquences n'atteignent jamais le
  terminal VTE) : écriture distant→local soumise à `r2l` + limite
  `osc52_max_bytes` (défaut 100 Ko) ; **lecture du presse-papier local par le
  distant (query) soumise à `l2r` et toujours auditée** (vecteur d'exfiltration —
  passez la politique à `r2l` ou `disabled` sur les hôtes non fiables).
- **Confirmation de collage** (réglage `paste_confirm_multiline`, actif par défaut) :
  tout collage multiligne ou contenant des caractères de contrôle/échappements
  ouvre un dialogue avec **aperçu exact** (␛ ␍ ␊ rendus visibles) — anti-pastejacking.
  Dialogue GTK natif. Annuler n'envoie rien.
- **Copy-on-select** optionnel (réglage) : la sélection part dans le presse-papier ;
  sélection primaire Linux native (clic milieu). Ctrl+Shift+C/V dans les deux moteurs.
- « Copier » un secret l'efface du presse-papier après 30 s (réglable). Attention :
  Klipper peut garder un historique — c'est inhérent à l'OS.
- Copier-coller de fichiers entre panneaux et drag & drop OS→app : livrés avec le panneau
  SFTP (§5.5).

### 5.5 SFTP et transferts (M5 — disponible)

**Ouvrir** : bouton **📁 Fichiers (SFTP)** de la barre du haut (bascule), ou bouton **Fichiers (SFTP)**
dans la barre d'état de la fenêtre terminal (ouvre le panneau sur CETTE session). **Il s'ouvre aussi
tout seul** à l'ouverture d'une session et suit la **session active** (terminal focalisé ou pastille
choisie dans la barre de sessions, §5.3). Le panneau est une
**vue intégrée, non modale** : il occupe la zone centrale de la fenêtre principale ; l'arbre des
connexions et la barre d'outils restent utilisables pour lancer d'autres sessions pendant que les
transferts tournent. « Fermer le panneau » (ou re-cliquer le bouton) le masque sans perdre l'état
(dossiers, sélection, transferts). Le panneau réutilise la connexion SSH du terminal (second canal,
sous-système `sftp`) : **aucune seconde authentification**. Sélecteur « Session » : sessions
connectées ; le changer ramène aussi le terminal correspondant au premier plan.

- **Deux panneaux** : Local (gauche) / Distant (droite) — fil d'Ariane cliquable, ↑ parent,
  ⟳, fichiers cachés (case), filtre, tri par nom/taille/date (clic sur l'en-tête),
  sélection multiple (Ctrl+clic, Maj+clic), double-clic = entrer dans un dossier (ou
  transférer un fichier). Les liens symboliques vers un dossier se parcourent.
- **Transférer** : boutons « Envoyer ➜ » / « ➜ Télécharger » (la destination est le dossier
  affiché de l'autre panneau), **glisser-déposer d'un panneau à l'autre**, ou **depuis le
  gestionnaire de fichiers vers la fenêtre** (envoi vers le dossier distant affiché).
  « Fichiers… » / « Dossier… » ouvrent le sélecteur natif.
- **Options** : si la destination existe → ignorer (défaut) ou remplacer ; liens
  symboliques → ignorer (défaut) ou suivre ; préserver droits et dates (défaut oui).
- **Opérations distantes** : nouveau dossier, renommer, droits (octal, récursif en option),
  lien symbolique, propriétés, copier le chemin, supprimer (dossier = récursif, confirmé).
- **Transferts** : file avec progression, débit, ETA, fichiers terminés ; parallélisme
  Réglages → « Transferts parallèles » (défaut 3, max 8) ; ⏸ pause, ▶ reprendre, ✕ annuler.
  Les dossiers sont développés récursivement (dossiers avant contenu).
- **Reprise après interruption** : les données arrivent dans `<fichier>.part`, renommé
  seulement à la fin (jamais de fichier à moitié écrit sous son vrai nom). Pause, erreur
  ou coupure réseau → ▶ (ou relancer le même transfert) repart de l'offset du `.part`,
  après comparaison des 64 derniers Kio des deux côtés : si la source a changé ou si le
  `.part` est corrompu, le transfert **repart de zéro**. Annuler supprime le `.part`
  du fichier en cours. Intégrité : la **taille** est vérifiée à la fin (SFTP v3 n'a pas
  de hash standard).
- **Barre de chemin** : au-dessus de chaque panneau, un champ éditable — tape un chemin puis
  Entrée (`/var/log`, `../tmp`, `~/docs`, `~` = dossier personnel ; relatif au dossier affiché) ;
  Échap restaure le chemin courant. Le fil d'Ariane cliquable reste disponible.
- **Suivre le terminal (OSC 7), façon MobaXterm** : case **Suivre le terminal** (en haut du
  panneau). La **cocher** active le suivi : si le shell de la session n'a pas encore annoncé son
  dossier, HarakaTS **tape une ligne** dans le shell (bash ou zsh) qui l'annonce à chaque prompt ;
  le panneau distant suit alors tes `cd`. Aucun fichier distant n'est modifié, l'effet s'arrête
  avec le shell. Fais-le **au prompt** (pas dans vim/htop/less : la ligne y serait tapée). **La
  ligne est masquée** : l'écho est rendu invisible (attribut SGR « conceal »), puis effacé dès que
  le shell répond ; il ne reste qu'un prompt normal (repli : si l'écran défilait ou si le prompt fait
  plusieurs lignes, il peut subsister un prompt vide en plus). Idempotent. **Automatique** : Réglages → « SFTP : activer automatiquement le suivi… » injecte
  cette ligne à chaque connexion/reconnexion d'une session native (désactivé par défaut). Pour un
  suivi permanent sans injection : *Options de transfert → Copier le snippet OSC 7*, à coller dans
  `~/.bashrc` / `~/.zshrc` du serveur. Fedora/Ubuntu avec `vte.sh` émettent parfois déjà OSC 7.
- **Disposition** : la vue est alignée sur l'arbre des connexions (même haut et même bas) et
  remplit la hauteur disponible sans défilement de la vue entière : barre du haut compacte
  (session, ⟳, suivi, *Options de transfert* repliées), deux panneaux dont les boutons d'action
  sont dans l'en-tête de chaque panneau, liste des transferts en bas (défilante).
- **Limites v1 assumées** : drag-out (panneau → gestionnaire de fichiers) non supporté
  par les webviews Tauri (utiliser « Télécharger ») ; transferts non pipelinés (un bloc de
  64 Kio par aller-retour : débit borné par la latence sur les liens lents) ; permissions
  des dossiers non préservées (fichiers : oui) ; `chown` non exposé ; extension `check-file`
  non utilisée ; le panneau local est en lecture seule (+ « Nouveau dossier »).
- Journal d'audit : `sftp_transfer` (sens), `sftp_remove` (jamais les chemins ni contenus).

### 5.6 Édition distante (M6 — disponible)

**Ouvrir** : dans le panneau SFTP, **double-clic sur un fichier distant** (≤ 5 Mio, sinon téléchargement),
ou sélectionner UN fichier puis **Éditer** (éditeur intégré) / **Éditer en externe**. Le bouton
**📝 Éditeur (n)** de la barre du haut affiche la vue Éditeur (non modale, même disposition que SFTP ;
plusieurs documents en onglets, l'arbre des connexions reste utilisable).

- **Éditeur intégré (CodeMirror 6)** : coloration selon l'extension du fichier (modules chargés à la
  demande), numéros de ligne, recherche/remplacement (Ctrl+F), historique (Ctrl+Z), indentation
  (Tab), parenthèses assorties, thème suivant celui de l'application. **Ctrl+S** enregistre : le
  fichier est renvoyé immédiatement sur le serveur (droits conservés).
- **Fins de ligne** : LF / CRLF conservées octet pour octet ; sélecteur dans la barre d'état pour
  convertir à l'enregistrement. Fichier non UTF-8 ou > 20 Mio : édition intégrée indisponible
  (éditeur externe possible pour les non UTF-8).
- **Conflit** : si le fichier distant a changé depuis l'ouverture (date/taille puis contenu SHA-256 ;
  un simple `touch` n'est pas un conflit), l'enregistrement est refusé et une barre propose :
  **Écraser** (impose votre version), **Recharger** (abandonne vos modifications, repart du distant) ou
  **Comparer** (vue côte à côte distant / local, éditable, puis « Enregistrer le résultat fusionné »).
  Fichier supprimé entre-temps : « n'existe plus » → Écraser le recrée.
- **Éditeur externe** : Réglages → **Éditeur externe** (ligne de commande ; `{file}` = fichier temporaire,
  sinon le chemin est ajouté ; **aucun shell** n'est utilisé ; vide = application par défaut du système
  via `xdg-open`). Chaque enregistrement dans l'éditeur est envoyé automatiquement (après ~0,4 s de
  calme) ; en cas de conflit, la même barre apparaît dans la vue Éditeur.
- **Envoi atomique** : écriture dans un fichier temporaire du même dossier, droits copiés, puis
  remplacement (SFTP ne permet pas d'écraser par renommage : l'ancien fichier est mis de côté puis
  supprimé, et restauré en cas d'échec). Aucun fichier partiel n'est visible sous son nom ; pendant
  quelques millisecondes le fichier peut être momentanément absent pour un lecteur concurrent.
- **Copies temporaires** : `<données>/tmp/<uuid>/` (répertoire 0700, fichier 0600), supprimées à la
  fermeture du document, au verrouillage du coffre et purgées au démarrage (crash). Les éditeurs
  externes peuvent créer leurs propres copies (swap, sauvegardes) : prudence avec les fichiers sensibles.
- Audit : `edit_open`, `edit_save`, `edit_conflict`, `edit_external` (jamais de chemin ni de contenu).

### 5.7 Tunnels, multi-exec, macros, journalisation, imports (M7 — disponible)

**Tunnels SSH (L / R / D)** — bouton **🔀 Tunnels (n)** (vue non modale, comme SFTP et l'éditeur).
- *Définition* : dans l'éditeur de connexion, section « Tunnels » : type **local** (`-L`, un port local
  atteint une destination via le serveur), **distant** (`-R`, un port du serveur atteint une destination
  de ton poste), **dynamique** (`-D`, proxy **SOCKS5** local). Adresse de bind **127.0.0.1 par défaut** ;
  une adresse non loopback expose le port sur le réseau (avertissement affiché, jamais bloqué).
  Case « démarrer automatiquement » : lancé à chaque (re)connexion de la session.
- *Live* : la vue liste les tunnels de toutes les sessions (état, port réel — le port 0 est attribué
  par l'OS/le serveur —, connexions actives, octets), **Démarrer / Arrêter à chaud**, ajout d'un tunnel
  ad hoc sur une session connectée. Fermer la session ou verrouiller le coffre arrête ses tunnels.
- *SOCKS5* : CONNECT uniquement (IPv4, IPv6, nom de domaine), sans authentification ; BIND et UDP
  refusés. Utilisation : `curl --socks5-hostname 127.0.0.1:<port> https://…`.
- *Remote* : dépend de `GatewayPorts` côté serveur pour un bind non loopback (conseil affiché).
- *Limites* : 256 connexions simultanées par tunnel, délais de 10–15 s (ouverture de canal,
  poignée de main SOCKS) ; un tunnel d'avant une reconnexion est coupé, seuls ceux en démarrage
  automatique reviennent ; `bind_addr` doit être une IP, `localhost` ou vide.
- Audit : `tunnel_start` / `tunnel_stop` (type et port, jamais la destination).

**Multi-exec** — bouton **Multi-exec** : coche ≥ 2 sessions connectées, **confirme**, la saisie (frappes,
collage, macros) d'une fenêtre membre est envoyée **octet pour octet et dans l'ordre** aux autres.
Indicateur permanent : étiquette « MULTI-EXEC (n) » dans la barre d'état de chaque fenêtre membre et
bandeau rouge dans la fenêtre principale. Le panneau est **flottant et non modal** (déplaçable par
l'en-tête, repliable, redimensionnable) : l'interface reste utilisable pendant qu'il est ouvert. Une
fois actif : barre « envoyer à toutes » (ligne + Entrée) et **envoi d'une macro existante**
(sélecteur ; aperçu exact et confirmation si la macro le demande ; la macro part vers un membre et est
diffusée aux autres, auditée comme tout envoi de macro). Se désactive tout
seul s'il reste < 2 membres (fenêtre fermée, session fermée, verrouillage). Les copies ne repassent pas
par la file d'entrée des autres sessions (pas de boucle).

**Macros / snippets** — bouton **Macros** : liste, création/édition (nom, contenu multiligne,
« confirmer avant envoi »). Le contenu est **chiffré dans le coffre** et masqué par défaut (Afficher =
action explicite auditée). **Envoyer** à une session depuis la fenêtre Macros ou via le **menu clic droit
du terminal** (sous-menu « Macros ») ; une macro marquée « confirmer » montre d'abord l'aperçu exact
(contrôles visibles). Chaque saut de ligne devient Entrée (la dernière ligne n'a Entrée que si le contenu se
termine par un saut de ligne). **Snippet de démarrage** (éditeur de connexion) : envoyé ~0,7 s après la
connexion, une fois par session ; une macro « confirmer » est **ignorée** au démarrage (note dans le terminal
+ audit) pour ne jamais bloquer sur un dialogue.

**Journalisation de session** (opt-in par connexion, case dans l'éditeur) : sortie du terminal écrite dans
`<données>/logs/<nom>-<AAAAMMJJ-HHMMSS>.log` (fichier **0600**, dossier 0700), séquences d'échappement
retirées, `\r` écrase la ligne (barres de progression), plafond 50 Mio, vidage toutes les 2 s. ⚠ Un
mot de passe tapé **peut** apparaître s'il est affiché en écho par la session. Les charges utiles OSC 52
n'atteignent jamais le journal. Audit : `session_log_start` / `session_log_stop`.

**Imports** — Export / Import → « Importer depuis une autre application » : **`~/.ssh/config`**, fichiers
**Remmina** (`.remmina`, un ou plusieurs, ou un dossier), **MobaXterm** (`.mxtsessions`). Aperçu avant
application (compteurs, connexions, avertissements, clés à passphrase), puis rapport. Tout atterrit dans un
groupe « Import <source> » à relire ; rien d'existant n'est modifié (les noms en conflit reçoivent un
suffixe). **Un import ne plante jamais** : entrées binaires, énormes, cycliques ou corrompues → rapport
d'avertissements.
- *ssh_config* : `Host` (motifs `*`/`?`, négation `!`), `Include` (glob, profondeur 8, cycles gardés),
  `HostName`, `User`, `Port`, `IdentityFile` (la clé est **importée dans le coffre**, un identifiant par
  fichier ; clé chiffrée → passphrase à ressaisir), `ProxyJump` (chaînes), `ServerAliveInterval`,
  `Local/Remote/DynamicForward` → tunnels. `Host *` → valeurs par défaut du groupe. **Non pris en charge,
  signalé** : `Match` (bloc ignoré), `ProxyCommand` ; les autres options sont listées dans un avertissement.
- *Remmina* : `protocol=SSH` uniquement, `server`, `username`, `name`, `group` (sous-groupes),
  `ssh_privatekey`, tunnel SSH → saut. Le mot de passe est chiffré par Remmina avec sa clé locale :
  **à ressaisir** (le déchiffrement via `remmina.pref` n'est pas implémenté).
- *MobaXterm* : sections `[Bookmarks*]`, `SubRep` → groupes imbriqués, sessions SSH (`#109#`) ; autres
  types ignorés (un avertissement) ; mots de passe **à ressaisir** ; clés embarquées non importées.
- Les secrets et contenus de clés ne transitent jamais vers l'interface (aperçu côté backend).

### 5.8 Export / import portable (M1, le critère n°1)

1. Réglages → Exporter : choisir la portée (tout / un dossier / sans secrets), une
   **passphrase d'export distincte du mot de passe maître**, le fichier `.hrkx`.
2. Sur l'autre poste : HarakaTS → Importer → sélectionner le `.hrkx`, saisir la
   passphrase d'export, choisir la stratégie de conflit (garder / remplacer / dupliquer).
3. Les secrets sont re-chiffrés avec la DEK du coffre local : toutes les connexions
   s'ouvrent avec leurs identifiants, sans ressaisie.
4. Transporte le bundle et la passphrase par des canaux séparés. Un bundle altéré
   (même d'un bit) est rejeté.

### 5.9 Verrouillage

- Auto-lock après inactivité (15 min par défaut, réglable), à la mise en veille et au
  verrouillage de session KDE. Au verrouillage : clés du coffre effacées de la mémoire, fichiers
  distants déchiffrés effacés et, par défaut, **toutes les sessions fermées**. Réglages → « Fermer
  toutes les sessions au verrouillage » (décoché) : les sessions SSH et les tunnels restent ouverts, les
  terminaux sont **masqués** pendant le verrouillage et reviennent au déverrouillage. Contrepartie :
  les connexions restent actives et les secrets déjà utilisés restent en mémoire jusqu'à leur fermeture.

### 5.10 Fenêtre principale

L'application démarre en **mode fenêtré** (1280×800 par défaut, minimum 940×600), jamais maximisée
d'office. À la fermeture, la taille (et l'état maximisé) est enregistrée dans
`window_state.json` (dossier de données) et restaurée au lancement suivant. Supprimer ce fichier
rétablit la taille par défaut. La position n'est pas mémorisée (Wayland la laisse au
gestionnaire de fenêtres).

### 5.10bis Apparence et accessibilité

- **Variables de design** (`ui/src/app.css`) : rayons (`--radius-*`), ombres (`--shadow-*`), tailles de texte
  (`--fs-*`), espacements (`--sp-*`), transition (`--t-fast`) et couleurs sémantiques par thème
  (`--warn`, `--fav`, `--alert`, `--ok-strong`, `--bad`, `--neutral`, `--term-bg`, `--overlay`). Plus aucune
  couleur en dur hors des définitions de thèmes : les 4 thèmes (Système / Gris / Bleu / Clair) s'appliquent à
  toute l'interface (pastilles d'état, bandeau multi-exec, favoris…).
- **Clavier** : anneau de focus visible (2 px, couleur d'accent) sur tous les éléments interactifs ; transitions
  courtes (140 ms) désactivées si le système demande un mouvement réduit (`prefers-reduced-motion`).
- **Contraste** : les couleurs de texte de chaque thème respectent WCAG AA (≥ 4,5:1 ; ≥ 3:1 pour les éléments
  graphiques). Le thème clair a été corrigé (`--muted`, `--accent`, `--ok` assombris). Le test
  `ui/src/lib/theme-contrast.test.ts` vérifie ces ratios pour les 5 jeux de couleurs.
- Les zones **natives** (onglets et barre d'état GTK, fenêtres de terminal) ne suivent pas encore ces thèmes.

### 5.11 À propos et licence

Bouton **ℹ À propos** (barre du haut) : version, plateforme, composants principaux (Tauri, russh,
russh-sftp, GTK+VTE, Argon2id / XChaCha20-Poly1305), dossier de données et **texte complet de la
licence MIT** du projet (fichier `LICENSE` à la racine, embarqué à la compilation et déclaré dans
les métadonnées Cargo et dans les paquets .deb/.rpm). Il n'y a plus de bandeau de version en bas
de la fenêtre ; les messages d'erreur s'affichent dans une **notification** en bas à droite
(fermable par ✕).

**Licences tierces** : bouton « Licences tierces » dans À propos → avis et textes complets des
composants Rust et npm intégrés (`THIRD-PARTY-LICENSES.md`, généré par
`cargo run -p xtask -- licenses` via cargo about + pnpm licenses, embarqué à la compilation et
bundlé dans les paquets). **Garde-fou CI** : `cargo deny check licenses` (`deny.toml`) — seules
les licences permissives compatibles MIT sont acceptées (MIT, Apache-2.0 ± LLVM-exception, ISC,
BSD, CC0, 0BSD, MIT-0, Unicode, Zlib, BSL, MPL-2.0 fichier) ; tout GPL/AGPL/inconnu fait échouer
la CI. Les libs système **LGPL** (GTK3, glib, VTE, WebKitGTK) sont liées **dynamiquement** via les
paquets distro — jamais en statique (obligation LGPL de relocabilité). Audit du 2026-11-21 :
aucun conflit avec la licence MIT du projet.

### 5.12 Langues (i18n)

**Langues livrées** : Français (référence), English, Italiano, Deutsch, Español, Português.
Réglages → **Langue** (liste générée automatiquement). Avant le déverrouillage du coffre (écran de
mot de passe), la langue est celle mémorisée au dernier choix, sinon celle du système si elle est
supportée, sinon le français ; après déverrouillage, c'est le réglage du coffre.

**Tout le texte est externalisé** : un fichier JSON par langue dans `ui/src/locales/<code>.json`
(clés plates `section.nom`, paramètres `{nom}`, pluriels `clé_one` / `clé_other`). Le même fichier
alimente l'interface web (Svelte) ET l'interface native GTK/VTE (clés `native.*`, embarquées à la
compilation par `src-tauri/src/i18n.rs`). Les messages d'erreur du backend (en français dans le
code Rust) sont traduits côté interface par `ui/src/lib/errors.ts` (une règle par message,
paramètres extraits) ; un message non répertorié reste affiché tel quel.

**Ajouter une langue** (ex. néerlandais `nl`) :
1. copier `ui/src/locales/fr.json` en `nl.json`, traduire les valeurs (jamais les clés ni les
   `{paramètres}`), mettre `_name` à « Nederlands » ;
2. ajouter une ligne `("nl", include_str!("../../ui/src/locales/nl.json"))` dans `LOCALES`
   (`src-tauri/src/i18n.rs`) — le test Rust `all_locale_files_are_registered` échoue sinon ;
3. lancer `pnpm --dir ui test` : `i18n.test.ts` vérifie que toutes les clés existent, qu'aucune valeur
   n'est vide et que les `{paramètres}` sont conservés. La langue apparaît seule dans les Réglages.
**Ajouter un texte** : créer la clé dans `fr.json` (puis dans les autres langues ; sinon repli
anglais → français), l'utiliser par `t("clé", {param})` / `tn("clé", n)`. Le test
`i18n-guard.test.ts` échoue si un composant Svelte contient un texte visible en dur.
**Limites** : les fenêtres de terminal natives gardent la langue en vigueur à leur ouverture ; les
textes renvoyés par le serveur distant (bannières, prompts d'authentification) ne sont pas traduits ;
les messages d'erreur Rust absents de `errors.ts` restent en français.

### 5.12bis Versions et publication

- Schéma **SemVer** : `0.N.0` à chaque jalon fonctionnel, `0.N.P` pour un correctif, `1.0.0` quand la
  recette §5.13 est validée en entier.
- Changer de version : `cargo run -p xtask -- version set 0.9.0` (modifie `Cargo.toml` puis aligne les
  JSON), `cargo build` (met à jour `Cargo.lock`), commit, puis `git tag -a v0.9.0 -m "…"` et
  `git push origin main --tags`. `version check` (CI) refuse tout désalignement.
- La version est visible dans « À propos » avec le hash git du build, et via `harakats --version`.
- Le format du coffre/export a sa propre version (v1, figée) : indépendante de celle de l'application.

### 5.12ter Branches git

| Branche | Rôle |
|---|---|
| `main` | **Stable** : seulement du code validé, chaque version publiée y est taguée `vX.Y.Z`. |
| `dev` | **Intégration** : on y fusionne les fonctions terminées avant de les valider ensemble. |
| `feat/<nom>` | **Une fonction ou un gros chantier** (ex. `feat/sftp-only`), créée depuis `dev`. |
| `fix/<nom>` | Correctif ciblé ; un correctif urgent part de `main` et est fusionné aussi dans `dev`. |

Flux : `feat/x` → `dev` (tests dans le conteneur) → `main` (version + tag + paquets). Petites
corrections et documentation : directement sur `dev` ou `main`. Retour arrière : `git checkout vX.Y.Z`
ou `git revert`. Les tags `stable-*` marquent des points de retour exceptionnels.

### 5.13 Checklist de recette v1 (poste propre)

À dérouler sur un poste Linux vierge (Fedora ≥ 42 / EL10+EPEL / Debian 12+ / Ubuntu 22.04+), KDE ou sans
keyring. Cocher chaque ligne ; toute case non cochée = anomalie à remonter avec `/tmp/harakats.log`
(`RUST_LOG=harakats=debug,info harakats 2>/tmp/harakats.log`).

**Installation**
- [ ] `rpm -qp --requires <paquet>.rpm` (ou `dpkg-deb -I <paquet>.deb`) liste `vte291` / `libvte-2.91-0` et WebKitGTK 4.1 ;
  la licence du paquet est MIT.
- [ ] Installation sans erreur (`sudo dnf install ./haraka-ts-*.rpm` ou `sudo apt install ./haraka-ts_*.deb`) ;
  l'entrée « HarakaTS » apparaît dans le menu des applications avec son icône.
- [ ] Premier lancement : création du coffre (mot de passe maître), aucune demande de keyring obligatoire
  (sans Secret Service/KWallet : message informatif seulement).
- [ ] Mode portable : un fichier `portable.marker` à côté du binaire (ou `--data-dir <dossier>`) place les données dans ce dossier.

**Coffre et portabilité (critère n°1)**
- [ ] Créer un groupe, une connexion et un identifiant (mot de passe + clé), **Export / Import → exporter tout** avec une passphrase.
- [ ] Sur un autre poste (ou `--data-dir /tmp/profilB`) : créer un coffre vierge, **importer** le bundle : tous les secrets
  sont retrouvés **sans ressaisie** (révéler/copier le mot de passe ; ouvrir une session avec la clé).
- [ ] Verrouiller puis déverrouiller ; auto-verrouillage après inactivité ; verrouillage ferme les sessions et vide `tmp/`.

**Sessions, terminal, SFTP, édition**
- [ ] Ouvrir une session (mot de passe, puis clé, puis 2FA/OTP si disponible) ; TOFU de la clé d'hôte puis alerte au changement.
- [ ] `htop`/`vim` : agrandir/réduire la fenêtre, quitter proprement ; copier/coller (Ctrl+Maj+C/V, clic milieu, collage multiligne confirmé).
- [ ] Jump host (connexion avec bastion).
- [ ] SFTP : parcourir, glisser-déposer un gros fichier, **pause puis reprise**, couper le réseau puis reprendre, dossier récursif.
- [ ] Éditer un fichier distant (éditeur intégré), enregistrer ; provoquer un conflit et tester Écraser / Recharger / Comparer ; éditeur externe.

**Sessions, onglets, coloration (0.8.0)**
- [ ] 2 connexions ouvertes : la barre de sessions affiche 2 pastilles ; cliquer ramène le terminal ; le panneau SFTP s'ouvre seul et suit le terminal actif ; ✕ ferme la session.
- [ ] Option « Terminaux en onglets » cochée, nouvelles sessions : arbre à gauche (240 px, réglable 200–420 px), onglets à droite, ✕ d'onglet ferme la session ; « Fichiers (SFTP) » ouvre le panneau (colonne large fixe) et sa fermeture rend la place au terminal.
- [ ] Verrouillage : par défaut les sessions se ferment ; case décochée, terminaux masqués puis restitués au déverrouillage.
- [ ] État des serveurs activé : pastilles verte/rouge dans l'arbre ; bulles ⓘ dans Réglages.
- [ ] Coloration : `journalctl -n 50` / `systemctl status sshd` colorent error/warning/`active (running)`, IP et URL ; `ls --color`, `vim`, `htop` inchangés ; case décochée = aucune couleur.

**Tunnels, multi-exec, macros, imports**
- [ ] Tunnel local (`-L`), distant (`-R`) et dynamique SOCKS5 (`curl --socks5-hostname 127.0.0.1:<port> …`) ; arrêt libère le port.
- [ ] Multi-exec sur 2 sessions : même saisie dans les deux, indicateur visible ; panneau flottant (interface utilisable derrière) ; macro existante envoyée depuis le panneau ; snippet de démarrage.
- [ ] Journalisation de session activée : fichier de log 0600 dans `<données>/logs/`.
- [ ] Imports : `~/.ssh/config`, un `.remmina`, un `.mxtsessions` → rapport sans plantage, connexions dans « Import … ».

**Langues et interface**
- [ ] Changer la langue (6 disponibles) : aucun texte français résiduel dans les autres langues.
- [ ] La fenêtre démarre fenêtrée et restaure sa taille au lancement suivant.

## 6. Emplacement des données

| Élément | Chemin (défaut) |
|---|---|
| Coffre | `~/.local/share/harakats/vault.hrk` (0600) |
| Sauvegardes rotatives (5) | `~/.local/share/harakats/backups/` |
| known_hosts interne | `~/.local/share/harakats/known_hosts` |
| Journal d'audit (sans secrets) | `~/.local/share/harakats/audit.log` |
| Journaux de session (opt-in) | `~/.local/share/harakats/logs/` |
| Temporaires d'édition | `~/.local/share/harakats/tmp/` (purgés) |

**Mode portable** (M7) : `portable.marker` à côté du binaire ou `--data-dir <chemin>` →
données dans `./data/` (clé USB, dépôt synchronisé). Le coffre étant chiffré, la
synchronisation cloud est sûre **si la passphrase est forte**.

## 7. Dépannage

| Symptôme | Cause / solution |
|---|---|
| `error while loading shared libraries: libwebkit2gtk-4.1.so.0` | libs runtime manquantes → bloc apt §3 |
| `dnf install` : « nothing provides webkit2gtk4.1 » | EL9 non supporté (voir §3) ; sur EL10, activer EPEL d'abord (`dnf install epel-release`) |
| `cargo tauri` : commande introuvable | le CLI est installé via pnpm à la racine : utiliser `pnpm tauri build` |
| « Permission denied » dans le conteneur sur `/work` | SELinux (hôte Fedora) : le montage bind doit porter `:z` (`-v …:/work:z`) |
| Corepack demande confirmation pour télécharger pnpm | variable `COREPACK_ENABLE_DOWNLOAD_PROMPT=0` (déjà passée dans le bloc de vérification M1) |
| « Secret Service indisponible » au démarrage | Normal si KWallet n'expose pas Secret Service : l'app continue, mot de passe maître requis. Cache désactivable dans les réglages |
| L'app ne démarre pas sous Wayland | Essayer `GDK_BACKEND=x11 ./harakats` ; terminal natif VTE (seul moteur) |
| Fenêtre terminal native absente / erreur VTE | Runtime `vte291` manquant → `sudo dnf install vte291` (Fedora) ou `libvte-2.91-0` (Debian/Ubuntu) |
| Binaire du conteneur incompatible hôte (`GLIBC_2.xx not found`) | hôte plus ancien que bookworm → reconstruire dans un conteneur aligné sur l'hôte (me le signaler) |
| Coffre corrompu (coupure pendant écriture) | au démarrage, proposition de restauration du dernier backup (`backups/`) |
| Mot de passe maître oublié | Aucune récupération possible (contrat de sécurité). Restaurer un export `.hrkx` si disponible |
| Transfert repris échoue | certains serveurs SFTP limitent les offsets → l'app repart de 0 avec avertissement |

## 8. Conseils de sécurité (utilisateur)

- Mot de passe maître = phrase de passe longue et unique ; c'est lui qui protège tout.
- Passphrase d'export **différente**, transmise par un canal séparé du bundle.
- Poste : swap chiffré recommandé, core dumps désactivés (`ulimit -c 0`) — le coffre
  déverrouillé vit en mémoire (voir SECURITY.md §8).
- N'active la journalisation de session que si nécessaire : la sortie terminal peut
  contenir des secrets echoés.
- Vérifie toujours les empreintes de host keys (TOFU) ; une alerte de changement de
  clé n'est jamais anodine.
- Le cache KWallet est un confort : sur poste partagé, laisse-le désactivé.

## 9. Historique des versions du guide

| Date | Jalon | Changements |
|---|---|---|
| 2026-11-20 | Phase 0 | Création : env hybride Podman+hôte, prérequis, structure du guide |
| 2026-11-20 | Phase 0 | Packaging : **.deb + .rpm** (AppImage retiré, décision utilisateur) ; cibles RPM Fedora ≥ 42 / EL10+EPEL / Mageia, EL9 exclu |
| 2026-11-20 | M1 | Docs validés par l'utilisateur ; M1 implémenté : coffre/crypto/export-import, tests normatifs, UI minimale (création/déverrouillage/verrouillage, réglages, export/import, audit), xtask (icônes+fixtures), CI. Build/tests à exécuter en conteneur (§4) |
| 2026-11-20 | M1 | **M1 vérifié vert** (run 9, agent autorisé) : clippy `-D warnings` propre, **35 tests Rust** (14 unitaires + 21 normatifs dont export A→B, falsification, AAD), svelte-check **0 erreur/0 warning**, vitest **4/4**, paquets `HarakaTS_0.1.0_amd64.deb` (3,73 Mio) + `HarakaTS-0.1.0-1.x86_64.rpm` (3,74 Mio) ; deps RPM vérifiées par `rpm -qp --requires` (webkit2gtk4.1, javascriptcoregtk4.1, gtk3, libsoup3, glib2) |
| 2026-11-20 | M2 | **M2 vérifié vert** : héritage avec provenance, CRUD arbre/credentials validé (anti-cycles, cascade, détachement), reveal/copy audité + TTL presse-papier (arboard 3.7), connexion rapide + IPv6, palette Ctrl+K (fuzzysort 4.0.2), i18n FR/EN, recalibration KDF, migration de schéma ; **46 tests Rust** (25 unit + 21 normatifs), vitest 6/6, svelte-check 0 erreur, paquets régénérés |
| 2026-11-20 | M2+ | Retours utilisateur testeur (app installée/lancée sur Fedora) : boutons suppression/verrouillage **fond gris**, « Dossiers » renommé **« Groupes »** (UI FR/EN), **barre de recherche des connexions** (floue, nom/hôte/user), **thèmes globaux Système/Gris/Bleu/Clair** (réglages, persistés dans le coffre) ; vérifié vert (46 tests Rust, 6 vitest, svelte-check 0/0), paquets régénérés |
| 2026-11-20 | M3 | **M3 vérifié vert** (8 runs) : terminal SSH complet — auth password/clé/clé chiffrée/OTP/agent, host keys TOFU + changement bloquant + import système, jump hosts chaînés, keepalive/latence/reconnexion, encodage, onglets/splits/détachement, Ctrl+Shift+C/V, prompts UI, known_hosts UI ; **63 tests Rust** (30 core + 21 normatifs + 11 SSH in-process + 1 interop openssh réelle via sshd conteneur), vitest 9/9, svelte-check 0/0, paquets 5,89/5,90 Mio |
| 2026-11-20 | M3+ | Retours utilisateur : **interface unifiée façon Remmina** (toolbar haut, arbre gauche, onglets sessions, dialogs pour éditeur/identifiants/réglages/export/audit, barre d'état bas, accueil avec récentes), **▶ par connexion + double-clic = connexion** + « ▶ Se connecter » en haut de l'éditeur + barre rapide connecte directement ; **diagnostic terminal vide** : compteur « ↓ N o » visible, logs backend (`RUST_LOG=harakats=debug,info`), fit robuste (rAF + timeout) + hauteur minimale ; vérifié vert, paquets régénérés |
| 2026-11-20 | M3++ | **Bug terminal vide CORRIGÉ** : log utilisateur `bytes=440 chars=0` → `encoding_rs::decode_to_string` n'agrandit pas le String (capacité = limite de sortie) ; `String::new()` avalait tout. Correctif : capacité via `max_utf8_buffer_length`, boucle sur octets non consommés, décodeur sans BOM sniffing + 2 tests de régression. **65 tests Rust**, 9 vitest, 0/0, paquets régénérés |
| 2026-11-20 | M3+++ | **Stabilisation terminal** (retours utilisateur : rétrécissement au lancement, saisie mal interprétée, top/htop désordonnés — cause : PTY serveur à une taille fausse car session ouverte avant le layout final) : ouverture de session seulement aux vraies dimensions du conteneur (ResizeObserver + fit préalable), resize écouté avant l'ouverture + debounce 100 ms, resynchronisation post-ouverture, taille mémorisée backend pour les reconnexions, saisie pré-connexion mise en file. Vérifié vert (65 tests, 9 vitest, 0/0), paquets régénérés |
| 2026-11-20 | M3-term2 | **Anti-gel terminal** (retours utilisateur : fenêtre qui rétrécit, htop/tmux figés et impossibles à quitter) : ① renderer **WebGL retiré** (instable sous WebKitGTK/Wayland → affichage figé) au profit du renderer DOM ; ② **sortie groupée par lots 16 ms / 32 Ko** (un message IPC par paquet SSH saturait la webview) ; ③ layout stable : terminal en position absolue dans une chaîne flex, barre d'onglets toujours visible, **fenêtre par défaut 1280×800 maximisée** (min 940×600). Recherche : Remmina SSH = VTE, MobaXterm = dérivé PuTTY, VS Code = xterm.js (notre bibliothèque). Vérifié vert (65 tests Rust, 9 vitest, 0/0), paquets 5,88 Mio |
| 2026-11-20 | M3-VTE | **Pivot moteur terminal (décision utilisateur)** : le log prouve que le backend envoie bien les écrans htop (lots 5,5 Ko/1,5 s) mais le rendu webview fige → **VTE natif (GTK), le moteur de Remmina, devient le défaut**. Fenêtre native 1100×700 **par session**, FFI direct libvte-2.91 (LGPL ; bindings Rust écartés : GPL ou GTK incompatible), mode feed sans PTY local (commit→SSH, size-allocate→window_change debounce 100 ms), prompts auth/host keys en dialogues GTK natifs (audités), fermeture fenêtre/verrouillage → sessions fermées. Réglage « Moteur de terminal » : VTE (défaut) / xterm web (repli, onglets/splits conservés). Deps : RPM `vte291` (déjà présent hôte), DEB `libvte-2.91-0`, build `libvte-2.91-dev` (conteneur + CI). Vérifié vert (65 tests Rust, 9 vitest, 0/0), paquets 5,93/5,94 Mio ; 4 runs (edition 2024 : `gen` réservé, `unsafe extern` ; gtk-rs 0.18 : `set_title(&str)`, `destroy()` unsafe, `invoke()` unitaire ; clippy : c"" littéraux) |
| 2026-11-20 | M3-VTE+ | **Revue ameliorations.md appliquée (§5.1–7)** : ① dimensions resize lues en idle (grille VTE à jour) + logs resize/connexions/fermeture + erreurs visibles en rouge dans le terminal ; ② **saisie/resize ordonnés** (canal mpsc + tâche unique par session, encodeur local) ; ③ **verrou global russh hors await réseau** (Arc<Channel>) ; ④ **feed coalescé** (1 invoke GTK en vol, tampon borné 16 Mo) ; ⑤ tests : rafale 50 resizes (dernière taille gagnante) + `stty size` contre vrai sshd + 4 tests keyinfo ; ⑥ **copier/coller natifs + menu contextuel** (clic droit), **barre d'état native** (état/latence/dims/**Reconnecter**), Annuler prompt = réponse vide ; ⑦ **import clé privée depuis fichier** (backend seul, métadonnées non secrètes, passphrase vérifiée avant enregistrement, .pub refusé, permissions averties, audité) ; ⑧ toolbar **Sessions** (liste + Focus + Fermer) ; ⑨ réglages police/scrollback VTE ; ⑩ variables d'env documentées (§4.1). Non appliqué : onglets VTE incrustés (choix utilisateur fenêtre native), import ssh_config (M7). Vérifié vert (**71 tests Rust**, 9 vitest, 0/0), paquets 5,97 Mio |
| 2026-11-20 | M3-VTE++ | **Vraie cause du gel htop/vim après agrandissement TROUVÉE et corrigée** (log `/tmp/harakats.log` : les lots de sortie s'arrêtent net après ~100 messages, avant/après le resize) : russh 0.65 livre chaque message du canal à `Handler::data` **et** à la file interne du `Channel` (bornée à `channel_buffer_size` = 100, `chan.send().await` dans la boucle de session, `client/encrypted.rs:611`). Personne ne lisait cette file → au ~100e message la boucle SSH se bloquait définitivement (plus de sortie, `window_change` jamais traité). Correctif : `channel.split()` ; moitié lecture drainée par une tâche dédiée, moitié écriture (`ChannelWriteHalf`) stockée dans `SessionEntry`. Test de non-régression `many_small_messages_do_not_stall_session` (400 échos d'1 octet + resize ; la version sans drain bloque). Vérifié : fmt, clippy `-D warnings` (workspace), 13 tests `haraka-ssh/ssh` verts. Les correctifs précédents (resize en idle, canal ordonné, verrou hors await, feed coalescé) restent valables mais ne traitaient pas cette cause |
| 2026-11-20 | M3-VTE++ | **Validation utilisateur** : htop/vim fonctionnent après agrandissement (build avec drain du canal russh). Gel clos. SFTP : aucun panneau pour l'instant (prévu M5) |
| 2026-11-20 | M4 | **Presse-papier et politiques — vérifié vert** : OSC 52 **intercepté dans le backend** (extracteur streaming avec carry inter-chunks, terminateurs BEL/ST, commun VTE/xterm — les séquences n'atteignent jamais le terminal) ; politique héritée `disabled/l2r/r2l/bidirectional` (effective + provenance) ; write soumis à `r2l` + `osc52_max_bytes` (100 Ko), **query (lecture du presse-papier local par le distant) soumise à `l2r` et toujours auditée** ; **confirmation de collage** multiligne/suspect avec aperçu exact (␛␍␊), anti-pastejacking (marqueurs bracketed paste retirés en bordure uniquement), dialogue GTK natif (VTE) + web (xterm), ordre de saisie préservé ; **copy-on-select** optionnel (VTE selection-changed + xterm onSelectionChange), sélection primaire native ; TTL secrets inchangé (M2). **Écart documenté** : copier-coller fichiers + drag & drop → M5 (dépend du panneau SFTP). Tests : 14 osc52 (chunks coupés/surdimensionné/query/base64 invalide), 6 paste, matrice politiques, 5 vitest miroir. **92 tests Rust, 14 vitest, 0/0**, paquets 6,00 Mio |
| 2026-11-20 | M4-fix | **Collage VTE corrigé** (retours utilisateur : dialogue de confirmation qui ne disparaît pas, Annuler le réaffiche ; Ctrl+Maj+C/V sans effet). Causes : ① la confirmation était déclenchée sur le signal `commit` de VTE, donc sur TOUTE saisie contenant un contrôle (flèches, Échap) et sur chaque morceau d'un collage (marqueurs bracketed paste `ESC[200~`/`ESC[201~` + texte = 3 `commit` → 3 dialogues en cascade) ; ② VTE ne câble pas Ctrl+Maj+C/V (c'est à l'application). Correctif : tout collage (Ctrl+Maj+V, Maj+Inser, clic milieu = PRIMARY, menu « Coller ») passe par `paste_from` (presse-papier GTK → confirmation unique si multiligne/suspect → `SessionCmd::Paste`) ; copie Ctrl+Maj+C / Ctrl+Inser ; les frappes ne sont jamais confirmées ; mode bracketed paste suivi dans le flux de sortie (DECSET 2004) et marqueurs internes retirés du contenu collé (anti-pastejacking) ; sans bracketed paste, `\n`→`\r` comme VTE. Vérifié : fmt, clippy `-D warnings`, tests workspace verts. Test manuel à refaire : Ctrl+Maj+V d'un texte multiligne (1 dialogue, Annuler = rien envoyé), flèches/Échap dans vim sans dialogue |
| 2026-11-20 | M3-VTE refactor | **Refactoring `vte.rs` (763 lignes, 1 fonction de 240 lignes à 8 paramètres) → module `src-tauri/src/vte/`** : `ffi.rs` (FFI libvte/pango + wrapper sûr `Term`, seul `unsafe` du module, invariants documentés), `handle.rs` (`VteWindow`, registre, helper unique `on_main_if_alive`), `window.rs` (assemblage), `clipboard.rs` (raccourcis, clic milieu, menu, collage), `resize.rs` (PTY initial + debounce), `dialogs.rs` (auth, clé d'hôte, confirmation de collage : construction `modal` + réponse `on_response` uniques), `mod.rs` (API + `VteOptions`). Comportement inchangé, API publique inchangée sauf `open_window(…, VteOptions)` ; plus de `#[allow(clippy::too_many_arguments)]`. Vérifié : fmt, clippy `-D warnings`, tests workspace verts |
| 2026-11-20 | M2-fix | **Favoris cliquables** (retour utilisateur : le groupe FAVORIS créé par l'étoile était inerte) : les lignes Favoris et Récentes ont maintenant les mêmes actions que l'arbre — double-clic / Entrée = connexion, ▶ Se connecter, ✎ Modifier, ☆/★ retirer/ajouter aux favoris (étoile jaune quand actif, aussi dans l'arbre). svelte-check 0/0, ESLint propre, vitest 14/14 |
| 2026-11-20 | M5 | **SFTP et transferts livrés** : `haraka-ssh` gagne `sftp` (listing, mkdir, rename, rm récursif, chmod récursif, liens, stat), `transfer` (file, workers parallèles 1–8, progression/débit/ETA, pause/reprise, **reprise `.part` avec contrôle des 64 Kio de queue**, récursif, droits/dates, conflits, liens ignorés/suivis, taille vérifiée) et `osc7` (suivi du répertoire courant, séquences coupées entre lots). Même connexion SSH que le terminal (`SessionManager::sftp`, second canal, ouverture mono-vol, invalidation sur erreur fatale). **Correctif critique** : `SshHandler::data` ne filtrait pas le canal → les octets SFTP auraient été injectés dans le terminal ; désormais seul le canal shell alimente l'affichage (test `sftp_bytes_never_reach_terminal`). `SSH_FXP_SYMLINK` : OpenSSH attend (cible, lien) — ordre constaté sur sftp-server. Tauri : 19 commandes (`sftp_*`, `local_*`, `transfer_*`) + événements `transfer://event`, `sftp://cwd`, `sftp://open` ; bouton SFTP dans la barre d'état VTE. UI : `SftpPanel`/`FilePane` (deux panneaux, glisser-déposer interne et depuis l'OS, actions CRUD, liste de transferts), badge de transferts actifs. **Tests** (interop OpenSSH réel) : CRUD/liens/chmod, aller-retour avec droits+mtime, **110 Mio : pause → reprise → coupure de la connexion → nouvelle session → reprise → fichier identique**, envoi 105 Mio pause/reprise, `.part` corrompu → redémarrage à 0, arbre récursif, 6 jobs / parallélisme ≤ 3, conflits skip/overwrite, permission refusée / chemins invalides ; unitaires osc7/sftp/transfer ; vitest `sftp.test.ts` |
| 2026-11-20 | M5+ | **Retours utilisateur** : ① **fenêtre principale en mode fenêtré** (plus de maximisation d'office), taille + état maximisé mémorisés dans `window_state.json` à la fermeture et restaurés (fenêtre créée invisible puis affichée après restauration, pas de clignotement ; bornes 940×600…10000) ; ② **suivi du terminal en un clic** (« Activer le suivi », façon MobaXterm) : `sftp_enable_follow` tape dans le shell une ligne idempotente bash/zsh (OSC 7, espaces et `%` encodés — vérifiée sous bash et zsh) ; réglage opt-in `sftp_follow_terminal` pour l'injecter à chaque (re)connexion ; le snippet à coller reste disponible ; ③ **barre de chemin éditable** dans les deux panneaux SFTP (`~`, relatif, `..` résolus, Échap). Vérifié : fmt, clippy `-D warnings`, tests `window_state`, vitest 22/22 |
| 2026-11-20 | M5++ | **Retours utilisateur** : ① section **Récentes** retirée (arbre et accueil) ; ② **SFTP non modal** : le panneau devient une vue intégrée à la zone centrale (monté une fois, masqué/affiché sans perte d'état, dépôts OS ignorés quand masqué), l'arbre et la barre d'outils restent utilisables ; ③ **activation du suivi invisible** : rediriger la sortie vers /dev/null n'aurait rien changé (la ligne affichée est l'ÉCHO du terminal, pas une sortie) — l'écho est donc masqué (SGR 8 avant l'envoi) puis effacé par mouvements relatifs (`ESC[nA`, `CR`, `ESC[J`, n = lignes occupées d'après la colonne du curseur VTE et la largeur) insérés juste après le premier OSC 7 reçu (`Osc7Scanner::first_end`), garde-fou 4 s, hook rendu idempotent ET toujours signalant (OSC 7 final). **Validé en pty réel + émulateur (pyte) sous bash et zsh, prompt en bas d'écran avec défilement** : un seul prompt restant, OSC 7 émis aux `cd`. Vérifié : fmt, clippy `-D warnings`, tests osc7, vitest 22/22, svelte-check 0/0 |
| 2026-11-20 | M5+++ | **Retours utilisateur** : ① bandeau de pied de page (version, id du coffre, dossier de données) **supprimé** — les erreurs passent dans une notification flottante ; ② **menu À propos** (`about_info` + `About.svelte`) avec infos de l'application et **licence MIT** (`LICENSE` créé, `license = "MIT"` dans Cargo, `bundle.license/licenseFile` dans `tauri.conf.json`) ; ③ **bas du panneau SFTP caché** (boutons Envoyer/Télécharger… derrière le pied de page) : pied supprimé + zone centrale défilante (`.content { overflow-y: auto }`) ; ④ log vérifié : aucune erreur applicative (échecs « connexion refusée » attendus vers un hôte sans sshd, reconnexion à backoff 2/4/8/16/30 s) ; l'avertissement `close session : session introuvable` à la fermeture d'une fenêtre dont la reconnexion a échoué était sans gravité → rétrogradé en debug |
| 2026-11-20 | M5++++ | **Retours utilisateur** : ① bouton « Activer le suivi » **supprimé** : la case « Suivre le terminal » l'active elle-même au clic (injection masquée, une fois par session tant qu'aucun OSC 7 n'a été vu) ; ② **vue SFTP réorganisée et alignée** : cause du décalage = règle globale `main { max-width; margin: 0 auto; padding: 24px 16px 64px }` appliquée au `<main class="content">` (neutralisée) ; panneau en carte alignée sur l'arbre (haut et bas), flex plein hauteur, boutons compacts dans l'en-tête de chaque panneau, options de transfert repliées, transferts en bas (150 px max, défilants), plus de défilement de toute la vue. Vérifié par **rendu visuel réel** (Chromium headless + maquette Tauri) à 1400×850 et 980×620 ; ce rendu a aussi révélé une accolade CSS orpheline (une `@media` non fermée avalait toutes les règles suivantes) — corrigée ; ③ log : session SFTP ouverte, aucune erreur |
| 2026-11-20 | i18n | **Internationalisation complète** (retours : bouton À propos mal rendu, anglais encore truffé de français) : ① langues **fr, en, it, de, es, pt** ; ② **un JSON par langue** (`ui/src/locales/`, 470 clés), découverte automatique, repli langue → anglais → français, `{paramètres}`, pluriels `_one/_other` (Intl.PluralRules), dates et nombres selon la langue ; ③ **source unique pour le web ET le natif GTK/VTE** (clés `native.*`, `src-tauri/src/i18n.rs`) : menus, dialogues (clé d'hôte, collage, authentification), barre d'état et messages écrits dans le terminal ; ④ **111 règles de traduction des erreurs backend** (`errors.ts`, branché sur `message()`) ; ⑤ langue mémorisée pour l'écran de déverrouillage (localStorage) + détection de la langue système ; ⑥ **garde-fous automatiques** : parité des clés/paramètres entre toutes les langues, aucun texte visible en dur dans les composants, toutes les langues déclarées côté Rust ; ⑦ bouton **« À propos »** sans glyphe ℹ. Travail parallélisé (5 agents d'extraction par zone de l'interface, 5 de traduction). Vérifié : fmt, clippy, tests Rust, svelte-check 0/0, ESLint, vitest 75/75, rendu visuel réel en allemand et anglais |
| 2026-11-20 | M6 | **Édition à distance livrée** + **moteur terminal unique (VTE)**. Édition : `haraka-ssh::edit` (téléchargement vers `tmp/<uuid>/` 0700/0600, référence mtime+taille+SHA-256, **conflit explicite** écraser/recharger/comparer, envoi par temporaire distant + remplacement avec sauvegarde/restauration, octets et CRLF/LF préservés, éditeur externe en argv SANS shell surveillé par `notify` avec débounce 400 ms et anti-boucle par hash, nettoyage à la fermeture / verrouillage / démarrage) ; 9 commandes `edit_*` + événement `edit://event` ; UI CodeMirror 6 chargée à la demande (`EditorView`, `CodeEditor`, `MergeCompare`, vue non modale), boutons Éditer / Éditer en externe, double-clic sur fichier distant, réglage Éditeur externe. **Tests d'interop OpenSSH réel** : droits 0700/0600 du temporaire, aller-retour avec droits conservés et aucun résidu `.tmp/.bak`, `touch` ≠ conflit, conflit puis les 3 résolutions, fichier supprimé, CRLF et non UTF-8 octet pour octet, refus > 20 Mio, **lecteur concurrent jamais face à un fichier partiel**, éditeur externe factice (envoi auto, conflit, pas de boucle), purge au démarrage ; unitaires argv/fins de ligne. **Retrait du moteur xterm** (demande utilisateur) : choix de moteur supprimé des Réglages, composants Terminal/SplitPane/tabs/paste et dépendances `@xterm/*` retirés (bundle 580 → 316 Ko), commandes IPC `session_open/reattach/write/resize`, `clipboard_read_text`, champ `terminal_engine`, fenêtres `detach-*` et capability de création de webview supprimés. i18n : 66 nouvelles clés × 6 langues |
| 2026-11-21 | M6-fix | **« Recharger » n'affichait pas le contenu rechargé** (retour utilisateur ; le fichier distant était bien à jour, et l'éditeur externe enregistrait correctement). Cause : « Recharger » remplace le contenu du store puis incrémente la révision, ce qui démonte/remonte l'éditeur CodeMirror ; au démontage l'ANCIENNE instance renvoyait une dernière fois son vieux texte (`flush`) et écrasait le contenu fraîchement chargé juste avant le remontage — le garde-fou lisait la révision réactive, encore périmée pendant le démontage (lot de réactivité de Svelte). Correctif : miroir NON réactif des révisions (`currentRev`) mis à jour immédiatement, garde-fou dans `EditorView` (envoi ignoré si la révision de l'instance ≠ révision courante), révision figée au montage dans `CodeEditor` pour le cache d'état. Même défaut corrigé pour le rafraîchissement après un enregistrement de l'éditeur externe. Reproduit puis vérifié **dans un vrai navigateur** (composant réel + IPC simulé) ; tests : `edit.svelte.test.ts` (store), svelte-check 0/0, ESLint, vitest 77/77 |
| 2026-11-21 | M7 | **Finalisation v1 : tunnels, multi-exec, macros, journalisation, imports, packaging.** *Tunnels* (`haraka-ssh::tunnel`) L/R/D + SOCKS5 maison, vue 🔀 + éditeur de specs, démarrage auto idempotent à chaque (re)connexion (`post_connect.rs`), arrêt à la fermeture de session / verrouillage ; **9 tests d'interop OpenSSH réel** (banner via -L, SOCKS5 domaine/IPv4/IPv6 + refus BIND/UDP, -R port 0 et fixe, conflits, 100 connexions, 20 Mio SHA-256, libération des ports) + unitaires. *Multi-exec* (`haraka-ssh::multiexec`) diffusion octet pour octet sans boucle, indicateur natif + bandeau, confirmation, auto-désactivation ; **test d'interop à 2 sessions réelles** (200 lignes en morceaux de 37 octets, même ordre). *Macros* : UI complète, contenu chiffré révélé seulement sur action auditée, envoi avec aperçu/confirmation, sous-menu clic droit natif, snippet de démarrage (macro « confirmer » ignorée). *Journalisation* opt-in (`haraka-core::sessionlog`, 0600/0700, ANSI retiré, plafond 50 Mio, après extraction OSC 52). *Imports* ssh_config / Remmina / MobaXterm (`haraka-core::import`, aperçu côté backend, clés importées dans le coffre, rapport, **fuzz déterministe 500 entrées sans panique**, fixtures anonymisées). *Packaging/CI* : métadonnées (éditeur, licence MIT, description), CI vérifie `rpm -qp --requires` (vte291, webkit2gtk4.1), licence et fichiers des paquets, audit npm ; **fixtures du format v1 figées** (xtask ne les régénère plus sans `--force`) ; checklist de recette §5.13 ; README final. i18n : 196 nouvelles clés × 6 langues |
| 2026-11-21 | Sécurité | **Durcissement (5 points)** : ① clé d'hôte changée = **blocage dur** (aucun bouton d'acceptation ; remplacement via Paramètres → Hôtes connus → Oublier, `knownhosts::forget`, audit) ; ② coffre : fsync du dossier, relecture de contrôle, rotation des sauvegardes seulement si le coffre est valide, contrôle d'intégrité au déverrouillage (sauvegardes valides/corrompues, restauration d'une corrompue refusée) ; ③ journaux de session **chiffrés par défaut** (`.log.enc`, XChaCha20-Poly1305, sous-clé dérivée de la DEK), lecture/suppression/purge dans Paramètres, rétention 30 j ; ④ temporaires d'édition dans `$XDG_RUNTIME_DIR` (tmpfs), écrasés puis supprimés ; ⑤ agent SSH : statut et identités visibles (clés FIDO2 `sk-` via l'agent). Tests : 2 nouveaux (journal chiffré/purge, store), test host key adapté |
| 2026-11-21 | Versions | **Nouveau versionnement SemVer aligné sur les jalons** : l'état M7 + durcissement devient **0.7.0** (tag `v0.7.0`), le SFTP seul sera 0.8.0, `1.0.0` = recette §5.13 validée. **Source unique** : `[workspace.package] version` de `Cargo.toml` ; `cargo run -p xtask -- version check\|sync\|set X.Y.Z` aligne `package.json` ×2 et `tauri.conf.json` (check en CI). **Hash git** dans « À propos » (`v0.7.0 · 7d2e042`, suffixe `-dirty` si arbre modifié) et `harakats --version` (build.rs → `HARAKA_GIT_HASH`) |
| 2026-11-21 | 0.7.1 | **Version 0.7.1** : créateur **LazySysAdmin** (README, « À propos », `LICENSE`, éditeur des paquets), `README.md` en anglais par défaut + `README.fr.md`, stratégie de branches (§5.12ter : `main` stable / `dev` / `feat/*`) |
| 2026-11-21 | 0.8.0 | **Version 0.8.0** (le SFTP seul passe en 0.9.0) : barre de sessions + SFTP lié à la session active (ouverture/connexion automatiques), terminaux en onglets (option expérimentale), multi-exec en panneau flottant avec envoi de macros existantes, coloration de la sortie du terminal (règles fixes), bouton « Sessions » supprimé |
| 2026-11-21 | Licences | **Conformité licences (audit MIT → implémentation)** : ① `THIRD-PARTY-LICENSES.md` généré par `cargo run -p xtask -- licenses` (**cargo about** pour Rust + **pnpm licenses list --json** pour npm, textes complets dédupliqués, versions réelles) — embarqué à la compilation (`include_str!`, commande `third_party_licenses`), bouton **« Licences tierces »** dans À propos (6 langues), bundlé dans les paquets (`bundle.resources`, vérifié `rpm -qpl`) ; ② **garde-fou `cargo deny check licenses`** (`deny.toml`, allowlist permissive MIT-compatible dont Apache-2.0 WITH LLVM-exception ; GPL/AGPL/inconnu = échec) en CI (job core) + séquence locale ; ③ image conteneur : cargo-deny + cargo-about (**feature `cli` requise ≥ 0.9**) ; ④ `about.toml`/`about.hbs`, séquence build_now.md et CI alignées. Audit : aucun conflit MIT (russh/aws-lc Apache/ISC → avis conservés ; GTK/VTE/WebKit LGPL en dynamique uniquement). Vérifié vert : **191 tests Rust, 111 vitest, 0/0, `licenses ok`**, paquets 0.8.0 7,92/7,93 Mio |
| 2026-11-21 | 1.0.0 | **Version 1.0.0 — première V1** : validation du porteur de l'interface (colonne de connexions réglable 200–420 px, onglets à droite avec point d'état, panneau SFTP à la demande avec suivi du dossier de chaque shell, menu ☰ unique, barre de connexion rapide et bouton « Nouvelle » retirés, option « Fermer les sessions au verrouillage » avec masquage des terminaux, état des serveurs verte/rouge, bulles d'aide ⓘ des Réglages, correctif du panneau qui disparaissait). Traductions anglaises des docs (`*.en.md`) à jour de 0.8.0 seulement |
| 2026-11-21 | 1.0.1 | **Correctifs et finitions 1.0.1** : séparateur arbre/terminaux sans scintillement aux limites 200/420 px (borne appliquée dans le signal `notify::position`, plancher natif), zone de dépôt « root » inutile retirée sous l'arbre (le déplacement passe par « Déplacer vers »), mention « (expérimental) » retirée du mode onglets, design tokens CSS (rayons, ombres, tailles, espacements, couleurs sémantiques), anneau de focus clavier global, `prefers-reduced-motion`, contrastes des thèmes clair/système vérifiés par test (vitest `theme-contrast`) |
| 2026-11-21 | 1.0.2 | **Sécurité et allègement 1.0.2** (audit qwen) : known_hosts **fail-closed** (fichier illisible = connexion refusée, ligne corrompue = clé changée), clés `@revoked` refusées (importées du known_hosts système), presse-papier **effacé au verrouillage** et copie de secret **exclue de l'historique Klipper** (indicateur `x-kde-passwordManagerHint`), copies de la clé du coffre, des réponses keyboard-interactive et du secret copié effacées de la mémoire, écriture atomique à nom de fichier aléatoire en création exclusive, dossiers de données créés en 0700, contrôle des permissions étendu (sauvegardes, journaux, audit, known_hosts) ; **poids** : binaire 21,2 → 14,6 Mo (LTO fat, opt-level s), paquet 7,97 → 5,97 Mio, suppression du polling de 2 s de la liste des sessions (événement `session://state`) |
| 2026-11-22 | 1.0.3 | **Nom de paquet unifié 1.0.3** : le paquet rpm s'appelle désormais `haraka-ts` (comme le deb), fichiers `haraka-ts-<version>-1.x86_64.rpm` et `haraka-ts_<version>_amd64.deb` ; `Obsoletes: HarakaTS` pour remplacer l'ancien paquet sans conflit ; l'entrée du menu des applications reste « HarakaTS » (modèle `.desktop` dédié) ; sections « Installation » ajoutées aux README fr/en ; traductions anglaises mises à jour (§5.10bis, 1.0.1, 1.0.2, SECURITY) |
| 2026-11-22 | 1.0.4 | **Correctif MikroTik / équipements réseau** : le hook de suivi du répertoire (syntaxe bash/zsh) n'est plus injecté que vers un serveur OpenSSH dont le prompt n'est pas de type `>` (il bloquait le CLI RouterOS, et pouvait gêner Cisco ou tout shell non POSIX) ; attente de la fin de la sortie avant injection ; **authentification** : message d'erreur détaillé (méthodes essayées / proposées), repli automatique sur une saisie de mot de passe puis keyboard-interactive si la méthode configurée est refusée |
| 2026-11-22 | 1.0.5 | **Correctif reconnexion** : `SessionManager::close`/`close_all` tenaient le verrou global pendant `disconnect().await` ; si la boucle russh ne répondait pas, toute nouvelle connexion restait bloquée (terminal vide après fermeture d'un onglet, constaté sur MikroTik/RouterOS). Le verrou est désormais relâché avant la déconnexion, qui a un délai maximum de 2 s |
| 2026-11-22 | 1.0.6 | **Retours visuels** : petit message « ✓ Enregistré / Supprimé / Copié / Terminé » (toast, 2,5 s, 6 langues) après les enregistrements, suppressions, copies et actions SFTP/macros/réglages |
| 2026-11-22 | 1.0.7 | **Correctif gel ~30 s après connexion** : le relevé de latence (`spawn_latency`) tenait le verrou global pendant `send_ping().await` ; RouterOS répond au ping par `SSH_MSG_UNIMPLEMENTED`, la réponse n'arrivait jamais et toute l'application (saisie, nouvelles connexions) se bloquait. Verrou relâché avant le ping, délai maximum de 5 s |
| 2026-11-22 | 1.0.8 | **Suppression du relevé de latence** (cosmétique) : plus de ping toutes les 30 s ni d'affichage « connecté — N ms » dans la barre d'état native ; le keepalive SSH (détection de coupure) est inchangé. Supprime à la source la cause du gel RouterOS corrigé en 1.0.7 |
| 2026-11-22 | 1.0.9 | **Hôtes connus : bouton « Tout effacer »** (réglages) : vide le known_hosts INTERNE de l'application (jamais `~/.ssh/known_hosts`), sauvegarde `known_hosts.BAK`, confirmation, audit `known_hosts_clear`, toast ; correctif du compteur « entrée(s) oubliée(s) » |
