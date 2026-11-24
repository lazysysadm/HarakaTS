# HarakaTS — Compte rendu et pistes d'amélioration

Date : 2026-11-20 — analyse statique du code (aucune exécution de l'interface graphique).
Périmètre lu : `src-tauri/src/{vte,sessions,main,state}.rs`, `crates/haraka-ssh/src/session.rs`,
`ui/src/lib/components/Terminal.svelte`, `ROADMAP.md`, `MODE_D_EMPLOI.md` (journal), `ARCHITECTURE.md`.

---

## 1. État du projet

- Application Tauri 2 (Rust + Svelte 5) : gestionnaire SSH/SFTP avec coffre chiffré portable.
- Livré : coffre, crypto, export/import (M1), arbre de connexions / héritage / credentials (M2),
  terminal SSH (M3).
- Pas encore livré : presse-papier avancé (M4), SFTP (M5), et suivants.
- Le terminal a changé trois fois de forme en une journée :
  1. xterm.js avec WebGL
  2. xterm.js avec rendu DOM et envoi par lots
  3. **fenêtre native GTK+VTE (moteur par défaut aujourd'hui)** : une fenêtre par session,
     FFI direct vers `libvte-2.91`.

Le bug décrit (gel avec htop/vim après agrandissement) concerne donc le **moteur VTE natif**.
Les correctifs précédents ciblaient la webview xterm, qui n'est plus le chemin par défaut.

---

## 2. Le bug : gel de htop/vim après agrandissement

Je n'ai pas pu reproduire le gel (pas d'affichage graphique ici). Ci-dessous, les causes
trouvées dans le code, de la plus probable à la moins probable. Elles sont à vérifier avec
la section 2.4.

### 2.1 Cause n°1 (certaine, bug réel) — dimensions lues trop tôt dans `size-allocate`

`vte.rs`, `connect_size_allocate` : le handler lit `vte_terminal_get_column_count/row_count`.
Or le signal `size-allocate` est de type *run-first* : le handler utilisateur s'exécute
**avant** le handler interne de VTE, qui recalcule la grille. On lit donc la taille
**précédente**.

Conséquences :

- Le PTY serveur peut s'ouvrir avec des dimensions fausses (80×24 par défaut de VTE).
- Chaque `window_change` envoyé est en retard d'une allocation. La dernière valeur envoyée
  après un agrandissement est donc **l'ancienne taille**.
- htop/vim dessinent pour une taille différente de celle de la fenêtre : écran tronqué, mal
  aligné ou apparemment figé. Le serveur ne reçoit jamais la vraie taille finale.

Correction : connecter le signal **après** le handler de classe (`connect_local("size-allocate", true, …)`,
c'est-à-dire `after = true`) ou, mieux, écouter `char-size-changed` et `resize-window`, puis lire
`get_column_count` après coup. Valider avec un test manuel : `stty size` après agrandissement
doit correspondre à la fenêtre.

### 2.2 Cause n°2 (probable) — saisie et resize non ordonnés

`sessions.rs`, `on_input` et `on_resize` lancent **une tâche tokio par événement**
(`tauri::async_runtime::spawn`). Le runtime est multi-thread : deux tâches lancées dans cet
ordre peuvent s'exécuter dans l'ordre inverse.

Conséquences :

- Caractères collés ou frappés rapidement dans le désordre (symptôme déjà noté dans le journal
  : « saisie mal interprétée »).
- `window_change` appliqué dans le désordre : l'ancienne taille peut gagner sur la nouvelle.
- Dans vim/htop, une séquence d'échappement coupée ou permutée laisse l'application dans un
  état incohérent.

Correction : un **canal mpsc par session** (`UnboundedSender<Cmd>`) et **une seule tâche
consommatrice** qui traite `Input(bytes)` et `Resize(c, r)` dans l'ordre. Cela supprime aussi
la création d'une tâche par touche.

### 2.3 Cause n°3 (possible) — verrou global tenu pendant un `await` réseau

`haraka-ssh/session.rs` : `write()` et `resize()` prennent `inner.lock().await` (verrou global
de **toutes** les sessions) et gardent le verrou pendant `channel.data_bytes(..).await` /
`channel.window_change(..).await`.

Conséquences :

- Si la file du canal russh est pleine ou la fenêtre SSH épuisée, tout verrou global reste
  pris : les autres sessions, `states()`, `close()`, `respond_auth()` se bloquent.
- Le handler `disconnected()` prend lui aussi `inner.lock()` : en cas de coupure pendant une
  écriture, risque d'**interblocage** (l'écriture attend la boucle russh, la boucle russh
  attend le verrou).
- Plus largement, toute l'application partage un seul verrou pour l'I/O de toutes les sessions.

Correction : au moment de `open`, stocker dans l'entrée un **handle d'écriture clonable**
(sender du canal) et ne tenir le verrou que le temps de le cloner, puis faire l'`await` hors
verrou. À combiner avec la correction 2.2 (une tâche d'écriture par session).

### 2.4 Cause n°4 (possible) — aucune contre-pression sur la sortie

`spawn_output_forwarder` pousse vers le thread GTK avec `glib::MainContext::invoke` par lot de
16 ms / 32 Ko, depuis un canal **non borné** (`unbounded_channel`). Si le thread principal est
occupé (redessin d'un VTE agrandi + webview Tauri sur la même boucle GTK), la file de
`feed` grossit sans limite. L'affichage prend alors du retard (impression de gel), puis la
mémoire monte. Quand vous agrandissez pendant que htop rafraîchit, VTE redessine et re-flow
tout l'écran, ce qui est le pire moment.

Corrections :

- Coalescer côté GTK : un seul `invoke` en vol par session, qui vide un tampon partagé
  (`Arc<Mutex<String>>`) au lieu d'un `invoke` par lot.
- Suspendre l'envoi de `window_change` et de sortie pendant le drag (déjà débouncé à 100 ms,
  garder), puis envoyer une seule fois à la fin.
- Mettre une borne (taille de tampon) et abandonner/aplatir les rafraîchissements
  intermédiaires de htop si le retard dépasse un seuil.

### 2.4 bis Cause n°5 (CONFIRMÉE par le log d'exécution) — file interne du canal russh jamais drainée

Mise à jour du 2026-11-20 après test utilisateur : htop s'arrêtait encore à l'agrandissement
alors que 2.1 à 2.4 étaient appliqués.

Preuve (`/tmp/harakats.log`, session réelle) : lots de sortie réguliers toutes les ~1,5 s
(htop), un dernier lot de 2 915 caractères juste après le `window_change`, puis **plus
aucune sortie**, même à la frappe. Le thread GTK restait vivant (le 2ᵉ resize et la fermeture
de fenêtre sont journalisés) : le blocage était dans la boucle SSH.

Mécanisme (code russh 0.65.2, `client/encrypted.rs:610-614`) : pour chaque `CHANNEL_DATA`,
russh fait `chan.send(ChannelMsg::Data).await` vers la file interne du `Channel`
(`channel_buffer_size` = **100**, bornée) puis appelle `Handler::data`. HarakaTS lit la sortie
dans `Handler::data` et ne lisait **jamais** la file du `Channel` → après ~100 messages
(htop en émet des dizaines par rafraîchissement), la boucle de session reste suspendue sur
`send().await` : plus de données reçues, `window_change` jamais traité. Ce défaut touchait
aussi le moteur xterm (« gel htop » historique).

Correctif (`crates/haraka-ssh/src/session.rs`) : `channel.split()` ; la moitié lecture est
drainée par une tâche (`while read_half.wait().await.is_some() {}`), la moitié écriture
(`ChannelWriteHalf`) sert à `write`/`resize`. Test : `many_small_messages_do_not_stall_session`
(400 messages d'un octet puis resize). Sans le drain, ce test se bloque.

### 2.5 À vérifier pour trancher (5 minutes)

1. Lancer avec `RUST_LOG=debug harakats` et regarder si, après le gel, des lignes
   `sortie terminal (lot)` continuent à apparaître :
   - oui → backend sain, problème de rendu/boucle GTK (causes 2.1 et 2.4) ;
   - non → backend bloqué (causes 2.2/2.3).
2. Après gel : dans une autre session, la saisie fonctionne-t-elle ? Si non, c'est le verrou
   global (2.3).
3. Tester avec `GDK_BACKEND=x11` pour exclure un problème Wayland, et avec
   `WEBKIT_DISABLE_COMPOSITING_MODE=1` pour exclure la webview Tauri.
4. Dans le terminal : `stty size` avant/après agrandissement, comparer avec les colonnes
   réelles (`tput cols`). Un écart confirme 2.1.
5. Vérifier si le gel est celui de **la fenêtre VTE seule** ou de **toute l'application**
   (fenêtre principale incluse). Toute l'application → boucle GTK saturée (2.4).

### 2.6 Test de non-régression à ajouter

Il manque un test du chemin resize. Ajouter, dans `crates/haraka-ssh/tests` (serveur russh
in-process, déjà présent) : ouvrir un PTY 80×24, enchaîner 50 `resize` rapides, vérifier que
le serveur reçoit **la dernière taille en dernier** et que l'écriture reste réactive.

---

## 3. Autres défauts relevés

| Gravité | Sujet | Détail |
|---|---|---|
| Moyenne | Copier/coller natif absent | `MODE_D_EMPLOI.md` annonce Ctrl+Shift+C/V « natifs ». `vte.rs` n'importe pas `vte_terminal_copy_clipboard_format` ni `paste_clipboard` et n'a aucun handler clavier ni menu contextuel. Le copier/coller n'est donc probablement pas fonctionnel. |
| Moyenne | Réglages ignorés | Police (`Monospace 11`), couleurs, scrollback (10 000), clignotement : tout est codé en dur. Les réglages de l'application ne sont pas appliqués au terminal natif. |
| Moyenne | Pas de recherche, pas de liens | Ni Ctrl+Shift+F ni liens cliquables dans la version VTE (disponibles seulement dans le repli xterm). |
| Moyenne | Reconnexion invisible | Les événements `state`/`closed` ne changent que le titre. Pas de bouton « Reconnecter » dans la fenêtre VTE, alors que la commande existe. |
| Faible | Timeout dimensions 5 s | Si la fenêtre n'a pas d'allocation, la session échoue sans message exploitable. |
| Faible | Fenêtres orphelines | Une fenêtre par session, non rattachée à la fenêtre principale : on les perd derrière d'autres fenêtres (voir section 4). |
| Faible | Prompts auth | Annuler ne répond rien : le serveur attend son timeout, la fenêtre reste « bloquée ». Envoyer une réponse d'annulation. |
| Faible | Encodage d'entrée | `encode_input` alloue `len*4+16` octets par frappe et prend un verrou `io` ; à remplacer par l'encodeur local à la tâche d'écriture (2.2). |
| Faible | Sécurité | `unsafe` + pointeurs bruts (`usize`) sur des widgets GTK : acceptable, mais à encapsuler dans un type unique pour éviter un `use after free` si le code évolue. |

---

## 4. Améliorations d'ergonomie (propositions)

### 4.1 Terminal (priorité haute — c'est le cœur de l'usage)

1. **Onglets dans la fenêtre principale** pour les sessions VTE. Option : intégrer le widget
   VTE dans la fenêtre Tauri (via `gtk::Box` autour de la webview) plutôt qu'une fenêtre par
   session. Évite la dispersion des fenêtres, garde Ctrl+Tab, glisser-déposer des onglets.
   Alternative plus courte : conserver une fenêtre par session mais ajouter une liste
   « sessions ouvertes » dans l'application principale avec focus en un clic.
2. **Barre d'état native** sous le terminal : état (connecté/reconnexion), latence (déjà
   mesurée par `Latency`), dimensions `cols×rows`, encodage, bouton Reconnecter.
3. **Reconnexion automatique visible** : bandeau « Connexion perdue — reconnexion (tentative
   2/5) » avec bouton Annuler. Le backend le fait déjà, il manque le retour visuel.
4. **Menu contextuel** (clic droit) : Copier, Coller, Tout sélectionner, Rechercher, Effacer,
   Réinitialiser le terminal, Ouvrir un SFTP ici.
5. **Collage sécurisé** : confirmation sur collage multiligne et aperçu (M4 prévu). Active par
   défaut pour limiter les erreurs.
6. **Zoom** Ctrl+molette et Ctrl+`+`/`-`/`0`, taille mémorisée par connexion.
7. **Thèmes et police** : liste de thèmes (Catppuccin déjà en usage, Solarized, Dracula,
   clair), choix de police et taille dans les réglages, appliqués aux sessions ouvertes en
   direct. Palette ANSI complète (aujourd'hui `NULL`, donc palette par défaut de VTE).
8. **Titre dynamique** : afficher `user@host — commande en cours` (OSC 0/2, signal
   `window-title-changed` de VTE) et la couleur du dossier/connexion dans la barre de titre
   pour distinguer production/test.
9. **Alerte de production** : couleur de bordure/fond propre à une connexion (ex. rouge pour
   prod), confirmation avant de coller `rm`, `shutdown`, `reboot`.
10. **Notification** à la fin d'une commande longue (bell ou OSC 777) quand la fenêtre n'a pas
    le focus.
11. **Scrollback** configurable, recherche dedans, export de la session en fichier texte,
    journalisation optionnelle (`script`-like) avec horodatage.
12. **Multi-exec** (taper dans N sessions) : déjà dans `ARCHITECTURE.md`, utile aux admins,
    avec un indicateur très visible pour éviter les erreurs.
13. **Snippets / macros** : le modèle de macros existe dans le store ; exposer un menu
    (Ctrl+Maj+M) pour insérer des commandes fréquentes dans la session active.

### 4.2 Gestion des connexions et navigation

1. **Palette Ctrl+K** (existe) : ajouter des actions (« reconnecter », « ouvrir SFTP »,
   « dupliquer la session », « verrouiller le coffre ») en plus de la recherche de connexions.
2. **Double-clic = ouvrir**, Entrée sur la sélection, glisser une connexion vers une zone
   pour créer un split. Vérifier que l'arbre est entièrement utilisable au clavier (critère
   d'acceptation M2).
3. **Connexion rapide** : accepter les formats `ssh://user@host:port`, `user@host -p 2222`,
   collés depuis le presse-papier ; proposer « enregistrer ».
4. **Import** depuis `~/.ssh/config` (Host, HostName, User, Port, ProxyJump, IdentityFile),
   Remmina, MobaXterm, PuTTY. C'est le principal frein à l'adoption.
5. **Tableau de bord** : « connexions récentes », « favoris », état en ligne (ping TCP en
   arrière-plan, pas bloquant), dernière connexion, tags filtrables.
6. **Restauration de session** au redémarrage : proposer de rouvrir les sessions de la veille.
7. **Auto-lock** : avertir 30 s avant le verrouillage, et **ne pas fermer les sessions
   actives** par défaut (le journal indique que le verrouillage ferme tout) ; option
   « conserver les sessions ouvertes pendant le verrouillage ».

### 4.2 bis Import de clé privée depuis un fichier (demandé)

**Constat.** `CredentialManager.svelte` (l. 153) n'offre qu'une `<textarea>` : l'utilisateur
doit ouvrir la clé dans un éditeur, copier, coller. Source d'erreurs (retours à la ligne
perdus, ligne `-----BEGIN` coupée, fichier mal copié) et la clé passe par le presse-papier
(historique Klipper). `rfd` est déjà une dépendance (`commands.rs` l'utilise pour l'import
`.hrkx`) : le sélecteur de fichier natif existe déjà côté backend.

**Proposition.** Bouton « Importer un fichier… » à côté de la zone « Clé privée », plus
glisser-déposer d'un fichier sur la zone.

Flux recommandé (respecte le principe « secrets jamais vers le frontend ») :

1. Le bouton appelle une nouvelle commande `credential_import_key_file(credential_id?)`.
2. Le backend ouvre `rfd::FileDialog` (dossier initial `~/.ssh`, filtre : tous fichiers,
   car les clés n'ont souvent pas d'extension : `id_ed25519`, `id_rsa`, `*.pem`, `*.key`).
3. Le backend lit le fichier **dans un tampon `Zeroizing`** et valide :
   - taille max (ex. 64 Ko) ;
   - texte UTF-8 contenant un en-tête `-----BEGIN ... PRIVATE KEY-----` (OpenSSH, PEM RSA/EC,
     PKCS#8) ; sinon message clair « ce fichier n'est pas une clé privée » ;
   - détection **clé publique** (`ssh-ed25519 AAAA…`, `.pub`) → refus avec explication ;
   - détection **clé chiffrée** (en-tête `ENCRYPTED`, ou `bcrypt` dans le format OpenSSH) :
     afficher « clé protégée par passphrase » et mettre le focus sur le champ passphrase.
4. Le backend tente de parser la clé (russh/ssh-key) pour renvoyer au frontend uniquement des
   **métadonnées non secrètes** : type (ed25519/rsa/ecdsa), taille, empreinte SHA256,
   commentaire, `encrypted: bool`. Le frontend affiche « ed25519 · SHA256:abc… · chiffrée ».
5. Le contenu de la clé reste côté backend dans un état temporaire (`pending_key`) et est
   écrit dans le coffre quand l'utilisateur clique Enregistrer. Rien de secret ne traverse
   l'IPC ; pas de passage par le presse-papier.
6. Fichier source : ne jamais le modifier. Proposer (case décochée par défaut) « supprimer
   le fichier d'origine après import » avec écrasement, et avertir si ses permissions sont
   trop larges (`0644` au lieu de `0600`).
7. Audit : entrée `credential_key_import` (nom du fichier, empreinte, **jamais le contenu**).

Améliorations liées :

- Si la clé est chiffrée et que la passphrase est saisie, **vérifier qu'elle déchiffre** avant
  d'enregistrer (test immédiat, message « passphrase incorrecte »).
- Importer aussi la clé publique voisine (`<fichier>.pub`) si elle existe, pour l'afficher
  et la copier vers `authorized_keys`.
- Import multiple : sélectionner plusieurs fichiers d'un coup et créer un credential par clé
  (nom = nom du fichier).
- Bouton inverse « Exporter la clé publique » et « Copier l'empreinte ».
- Même mécanisme pour la **connexion** : champ « Fichier de clé » qui importe au lieu de
  référencer un chemin (garde la portabilité : le chemin local ne suivrait pas l'export).
- Tests : vitest pour l'état du formulaire ; tests Rust sur fixtures (clé ed25519 claire,
  clé chiffrée, RSA PEM, clé publique, fichier binaire, fichier trop gros, CRLF).

Pas besoin du plugin `tauri-plugin-fs` ni d'élargir les capabilities : la lecture se fait
côté Rust, la surface d'attaque de la webview ne change pas.

### 4.3 Retour utilisateur et erreurs

1. Messages d'erreur actionnables : au lieu de `coffre verrouillé` ou `connexion introuvable`,
   expliquer quoi faire (déverrouiller, vérifier l'hôte).
2. Distinguer les échecs : DNS, port fermé, délai dépassé, authentification refusée, clé
   d'hôte changée. Un message court par cas avec le bouton adéquat (modifier, réessayer).
3. Progression visible à l'ouverture : « Connexion à host… → Authentification (clé) →
   Terminal prêt » (les événements `State` existent déjà).
4. Indicateur de latence et de perte de réseau (barre d'état), pas seulement dans le titre.

### 4.4 Qualité et robustesse

1. **Mode diagnostic** dans les réglages : bouton « Copier les journaux » (sans secrets) et
   niveau de log, pour traiter les gels comme celui-ci sans `RUST_LOG`.
2. **Chargement différé** du code CodeMirror/xterm (le bundle fait 549 Ko, l'avertissement
   Vite est déjà présent) : `import()` dynamique.
3. **Chaîne de test automatisée du terminal** : serveur russh in-process + test qui lance
   `top`-like (flux rapide), redimensionne en boucle, vérifie qu'aucun message n'est perdu ni
   réordonné.
4. **Retirer le code mort** si le repli xterm n'est plus supporté (ou le garder, mais alors
   le tester dans la CI) pour ne pas maintenir deux moteurs à moitié finis.
5. **Documenter les variables d'environnement utiles** (`GDK_BACKEND`, `RUST_LOG`,
   `WEBKIT_DISABLE_COMPOSITING_MODE`) dans `MODE_D_EMPLOI.md`.

---

## 5. Ordre de travail recommandé

1. **Corriger 2.1** (size-allocate après le handler de classe) : petit changement, effet
   immédiat probable sur htop/vim.
2. **Corriger 2.2 + 2.3** : une tâche d'écriture ordonnée par session, handle d'écriture hors
   verrou global.
3. **Corriger 2.4** : un seul `feed` en vol, tampon coalescé.
4. Ajouter le **test de resize en rafale** (2.6) pour verrouiller le correctif.
5. Implémenter le **copier/coller natif** et le **menu contextuel** (section 3, 4.1).
6. Barre d'état + reconnexion visible + thèmes/police appliqués (4.1).
7. **Import de clé privée depuis un fichier** (4.2 bis) : petit, utile tout de suite.
8. Intégration en **onglets** dans la fenêtre principale (4.1.1), plus grosse refonte.
9. Import `~/.ssh/config` (4.2.4) pour faciliter l'adoption.

## 6. Limites de cette analyse

- Lecture statique du code uniquement : aucune exécution, aucun test lancé, aucun log de
  gel disponible.
- Les causes 2.3 et 2.4 sont plausibles mais non confirmées ; seule 2.1 est un défaut
  déterministe lisible dans le code. Les vérifications de 2.5 permettent de trancher.
- Le comportement exact du signal `size-allocate` de VTE (ordre des handlers) doit être
  confirmé par un test manuel (`stty size`) avant et après correctif.

---

## Suivi — durcissement sécurité (2026-11-21)

Points 1 à 5 de la liste de priorités traités : clé d'hôte changée bloquante sans acceptation possible
(oubli explicite dans Paramètres), intégrité du coffre (fsync dossier, relecture, rotation conditionnelle,
détection de corruption au déverrouillage), journaux de session chiffrés + rétention, temporaires
d'édition en tmpfs écrasés à la suppression, statut de l'agent SSH (clés FIDO2 `sk-` via l'agent).
Détails : `SECURITY.md` §7.1–7.4 et `MODE_D_EMPLOI.md` (historique).
