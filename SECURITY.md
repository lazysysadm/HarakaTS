# SECURITY — HarakaTS v1

> Phase 0 — modèle de menace et politique de sécurité. À valider avant écriture de code.
> Compléments : [EXPORT_FORMAT.md](EXPORT_FORMAT.md) (format binaire), [ARCHITECTURE.md](ARCHITECTURE.md) §5.

## 1. Modèle de menace

| # | Scénario | Protection | Résultat attendu |
|---|---|---|---|
| T1 | **Vol du fichier de coffre** (disque, backup, cloud, clé USB) | Argon2id (paramètres calibrés 0,5–1 s) + XChaCha20-Poly1305 ; DEK 256 bits aléatoire ; aucun secret en clair sur disque | Attaque hors-ligne = coût Argon2id par essai ; sans le mot de passe maître, rien n'est déchiffrable. Le keyring OS n'ajoute **aucune** voie de déchiffrement hors du poste |
| T2 | **Poste compromis, coffre déverrouillé** (malware utilisateur) | Limites assumées : la DEK et les secrets sont en mémoire processus ; zeroize/secrecy réduisent la fenêtre mais un attaquant même utilisateur peut lire la mémoire | **Non couvert** (voir §8). Atténuations : auto-lock inactivité/veille/verrouillage session, keyring cache désactivé par défaut, reveal/copy journalisés |
| T3 | **Export intercepté** (réseau, mail, partage) | Bundle re-chiffré avec une **passphrase d'export distincte** du mot de passe maître, Argon2id + XChaCha20-Poly1305 ; option « sans secrets » | Sans la passphrase d'export, le bundle est inutile ; le mot de passe maître n'est jamais réutilisé ni transmis |
| T4 | **Falsification d'un export** (bit flip, remplacement de blocs, permutation de secrets) | AEAD sur l'enveloppe entière (AAD = en-tête canonique) + AEAD par secret (AAD = vault_id ∥ entry_id ∥ champ) | Toute altération → échec du tag → **rejet complet**, aucun déchiffrement partiel. Permuter deux secrets entre entrées/champs échoue aussi (AAD) |
| T5 | MITM SSH (usurpation d'hôte) | TOFU avec empreinte SHA256 affichée, known_hosts interne, **alerte bloquante** si la clé change, aucun accept automatique | L'utilisateur décide explicitement ; changement de clé = refus par défaut + audit |
| T6 | Collage malveillant (pastejacking : commandes cachées dans un collage) | Bracketed paste, confirmation multiligne, détection de caractères de contrôle/échappements, aperçu exact du contenu | L'utilisateur voit ce qui partira avant l'envoi |
| T7 | Fuite via l'UI / IPC / logs | Secrets jamais envoyés au frontend sauf reveal/copy explicite ; DTOs sanitisés ; `secrecy` (Debug rédigé) ; logs backend sans matériaux d'auth ; CSP stricte, aucune URL distante | Pas de secret dans les événements, les panics, les dumps ou les logs |
| T8 | Presse-papier persistant (historique Klipper/GNOME) | Effacement automatique après N s (défaut 30) | **Limite assumée** : un gestionnaire d'historique peut avoir capturé la valeur (§8) |
| T9 | Abus du keyring OS | Cache **opt-in**, désactivé par défaut ; la DEK wrappée par mot de passe maître reste toujours dans l'en-tête | Sans Secret Service (KWallet absent), l'app fonctionne ; le keyring n'est jamais requis pour déchiffrer |

## 2. Hiérarchie de clés et chiffrement

```
mot de passe maître ──Argon2id(sel, m, t, p)──▶ KEK (256 bits)
                                                 │ wrap (XChaCha20-Poly1305, AAD="HRKTS/DEK"∥vault_id)
                                                 ▼
DEK (256 bits, OsRng) ───────────────────────────┤
   │                                             │
   ├─ couche 1 : payload entier du store         │  AAD = magic ∥ version ∥ header_len ∥ header
   │            (JSON : connexions, dossiers,    │
   │             credentials, métadonnées)       │
   │                                             │
   └─ couche 2 : chaque champ secret             │  AAD = vault_id ∥ entry_id ∥ field_name
                (password, private_key,          │  (canonique, préfixes de longueur u32be)
                 passphrase, contenu de macro)   │
                nonce 24 o aléatoire par chiffrement
```

- **Double couche volontaire** : la couche 1 protège la confidentialité des métadonnées
  (hôtes, utilisateurs, topologie — sensibles pour un sysadmin) ; la couche 2 lie chaque
  secret à son entrée et son champ (anti-permutation, exigence du prompt) et permet
  l'export « sans secrets » ainsi que la ré-encryption champ par champ à l'import.
- **Aléatoire** : `rand 0.10` OsRng (rand_core 0.10, aligné avec aead 0.6 — aucun mélange
  de lignées rand_core). Nonces XChaCha20 : 24 octets aléatoires uniques par message
  (sûr jusqu'à ~2^80 messages par clé ; rotation de DEK possible post-v1).
- **KDF** : Argon2id v0x13, API brute `argon2 0.6::hash_password_into`. Calibration à la
  création : m = 131072 KiB (128 MiB), p = 4, t ajusté (2→8) pour atteindre 0,5–1 s ;
  si la machine est lente, m réduit par paliers (plancher 64 MiB) ; paramètres stockés
  dans l'en-tête, re-calibrables. Sel 16 octets aléatoire, **nouveau sel à chaque
  changement de mot de passe maître**.
- **Changement de mot de passe maître** : vérification de l'ancien (unwrap DEK), nouveau
  sel + nouvelle KEK, **re-wrap de la DEK uniquement** ; payload et secrets inchiffrés
  tels quels → opération O(1), pas de réécriture du coffre.
- **En mémoire** : DEK dans `SecretBox<[u8; 32]>`, mots de passe/passphrases dans
  `SecretString`/`SecretBox`, buffers de déchiffrement `zeroize` après usage (y compris
  les copies temporaires). `Debug`/`Display` rédigeés via `secrecy`. Aucun secret dans
  les messages d'erreur : les erreurs crypto sont typées (`WrongPassword`, `Tampered`,
  `UnsupportedVersion`) sans contenu sensible.

## 3. Keyring OS : cache de confort, jamais une dépendance

- Backend : `keyring 3.6.2`, features `sync-secret-service` + `crypto-rust` + `vendored`
  → Secret Service D-Bus (KWallet sous KDE, GNOME Keyring, KeePassXC…).
- Contenu : la DEK brute (`set_secret`, binaire), entrée `service="harakats"`,
  `user=<vault_id>`. **Conséquence assumée** : sur ce poste, session ouverte, un process
  capable d'interroger le Secret Service peut déverrouiller le coffre sans le mot de passe
  maître. C'est le prix du confort — désactivé par défaut, activable explicitement,
  désactivable à tout moment (l'entrée est supprimée).
- **Jamais indispensable** : l'en-tête contient toujours la DEK wrappée par la KEK
  maître. Absence de service, service verrouillé, erreur D-Bus, KWallet non configuré →
  repli **silencieux et non bloquant** sur le mot de passe maître (exigence utilisateur).
  Aucun message d'erreur bloquant au démarrage si le keyring est absent.
- Changer le mot de passe maître ne change pas la DEK : le cache keyring reste valide
  (documenté dans l'UI). Supprimer le cache = `delete_credential`.
- Le keyring ne stocke **rien d'autre** (ni mots de passe de connexions, ni passphrase
  d'export) : tout le reste vit dans le coffre.

## 4. Hygiène des secrets et IPC

- Sens interdit : backend → frontend, sauf `credential_reveal(id, field)` et
  `clipboard_copy_secret(id, field, ttl)` — actions utilisateur explicites, journalisées
  (audit), valeur renvoyée une seule fois, jamais mise en cache côté UI.
- Sens autorisé : frontend → backend quand l'utilisateur saisit un secret (formulaires
  credential, mot de passe maître, passphrase d'export, réponses keyboard-interactive).
  Ces valeurs transitent par l'IPC Tauri (canal local, pas de réseau) et sont
  immédiatement enveloppées dans `secrecy` côté backend.
- Le backend s'authentifie **lui-même** : russh reçoit les matériaux depuis le
  `SecretResolver` en mémoire, jamais depuis l'UI.
- Presse-papier : copie d'un secret → effacement automatique après N secondes (défaut 30,
  réglable, 0 = pas d'effacement avec avertissement). La sélection primaire Linux est
  effacée aussi.
- **OSC 52 (M4)** : intercepté dans le backend avant le terminal (VTE natif ou xterm) —
  aucune séquence OSC 52 n'atteint le renderer. Écriture distant→local : politique
  `r2l`/`bidirectional` + limite `osc52_max_bytes` (défaut 100 Ko), auditée (taille,
  jamais le contenu). **Query (le distant lit le presse-papier local) : politique
  `l2r`/`bidirectional`, toujours auditée** — vecteur d'exfiltration ; sur hôte non
  fiable, politique `r2l` ou `disabled`. Les opérations refusées sont journalisées.
- **Collage (M4)** : multiligne ou caractères de contrôle/échappements (hors marqueurs
  bracketed paste en bordure) → confirmation avec aperçu exact (contrôles visibles),
  anti-pastejacking ; un marqueur bracketed paste interne est traité comme suspect.
  Annuler n'envoie rien. Réglage `paste_confirm_multiline` (défaut actif).
- Logs : `tracing` avec champs secrets rédigeés ; aucune valeur de secret, ni DEK, ni
  passphrase dans les logs, erreurs, panics. Les panics sont interceptées (hook) pour
  journaliser un message générique. Les dumps/core dumps ne sont pas désactivables par
  l'app — limite assumée (§8).
- Journalisation de **session terminal** (opt-in par connexion, désactivée par défaut) :
  le flux brut peut contenir des secrets echoés par le shell distant (ex. mot de passe
  tapé en clair). Avertissement explicite à l'activation ; fichiers 0600 ; purge
  configurable. Aucun matériau d'authentification backend n'y est écrit.

## 5. Durcissement Tauri / webview

- CSP stricte (ARCHITECTURE.md §13) : `script-src 'self'`, aucune URL distante,
  `object-src 'none'`, `frame-ancestors 'none'`. Styles inline autorisés uniquement
  (xterm/CodeMirror injectent des styles dynamiques).
- Capabilities minimales : commandes déclarées une à une, `core:event`, création de
  fenêtre pour les onglets détachés. **Aucun plugin fs/shell/http/dialog/opener**.
  Dialogues fichier : `rfd` côté backend. Éditeur externe : `std::process::Command`
  côté backend, commande configurable par l'utilisateur (pas d'injection shell :
  exécution directe argv, jamais `sh -c` sur une chaîne interpolée).
- `devtools` uniquement en build dev. `dangerousDisableAssetCspModification` : non.
- Validation de toutes les entrées IPC (types serde + bornes : tailles, chemins, UUID) ;
  les chemins SFTP/locaux sont normalisés côté backend ; aucune traversée de répertoire
  autorisée pour l'édition distante hors du temp privé.
- Drag & drop OS → app : uniquement des chemins de fichiers locaux, jamais d'URL
  chargées dans la webview.

## 6. Fichiers et permissions

| Chemin | Mode | Contenu |
|---|---|---|
| `data/` (répertoire) | 0700 | tout le reste |
| `vault.hrk` (+ `.tmp`, backups) | 0600 | en-tête + payload chiffré |
| `known_hosts` | 0600 | clés publiques d'hôtes (non secret, intégrité importante) |
| `audit.log` | 0600 | événements sensibles, sans secrets |
| `logs/<session>.log` | 0600 | journalisation opt-in |
| `tmp/<uuid>/` | 0700 (fichiers 0600) | copies temporaires d'édition distante, purgées à la fermeture et au démarrage |

- Écritures atomiques (tmp + fsync + rename) ; umask vérifié au démarrage ; si le
  répertoire de données existe avec des permissions trop larges → avertissement +
  correction proposée (jamais silencieuse).
- Mode portable : mêmes permissions sur le support amovible (avertissement si le FS ne
  les supporte pas, ex. FAT).

## 7. Sécurité SSH

- Host keys : TOFU + empreinte SHA256 format OpenSSH ; **changement de clé = blocage dur** :
  la session est refusée sans aucun bouton d'acceptation (aucun prompt répondable n'est créé) ; le
  remplacement passe par l'**oubli explicite** de l'entrée (Paramètres → Hôtes connus → Oublier,
  audit `known_hosts_forget`), puis une nouvelle première connexion TOFU ; import du
  known_hosts système (entrées hashées et marqueurs `@revoked` supportés : une clé révoquée est refusée
  sans prompt). **Fail-closed** : un known_hosts illisible (erreur d'E/S, octets invalides) refuse la
  connexion au lieu de retomber sur une première connexion TOFU ; une ligne corrompue pour l'hôte
  compte comme une clé changée. Aucun `StrictHostKeyChecking=no` implicite, aucune option
  « toujours accepter ».
- Algorithmes : `russh::Preferred` par défaut (kex modernes, ed25519/ecdsa/rsa-sha2) ;
  pas d'algorithmes faibles réintroduits manuellement.
- Clés privées : importées **dans le coffre** (chiffrées), jamais laissées en clair dans
  le temp ; les clés chiffrées sont acceptées (passphrase dans le coffre).
- Agent SSH local : lecture via `SSH_AUTH_SOCK` pour l'authentification (essayé en premier ; la liste
  des identités, clés publiques seulement, est visible dans Paramètres) ; les **clés matérielles FIDO2
  (`sk-…`)** fonctionnent via l'agent : la clé privée ne quitte jamais le jeton et rien n'entre dans le
  coffre ; **agent
  forwarding désactivé et non proposé en v1** (un serveur distant compromis pourrait
  utiliser l'agent pour rebondir).
- Keyboard-interactive : les prompts sont affichés tels quels (bannière serveur incluse,
  rendue en texte brut — jamais d'HTML/URI cliquable automatique) ; les réponses
  (OTP) ne sont jamais journalisées.
- Tunnels : bind par défaut `127.0.0.1` (pas `0.0.0.0`) ; exposer un tunnel remote dépend
  du `GatewayPorts` serveur, affiché dans l'UI ; SOCKS5 dynamique sans authentification
  → bind loopback uniquement par défaut, avertissement si bind non-loopback.
- Reconnexion automatique : uniquement coffre déverrouillé ; au lock, les sessions sont
  fermées (défaut) et les matériaux zeroizés.

### 7.1 SFTP et transferts (M5)

- **Même connexion, même authentification** : le canal SFTP est un second canal de la session
  SSH du terminal ; aucun secret n'est ressaisi ni ré-exposé au frontend.
- **Isolation des flux** : seul le canal shell alimente l'affichage du terminal (filtrage par
  identifiant de canal dans le handler) — une réponse SFTP ne peut ni apparaître ni être
  interprétée comme séquences d'échappement par le terminal. Test de non-régression dédié.
- **Chemins** : toujours transmis tels quels au protocole SFTP (jamais via un shell) ; aucun
  chemin ni contenu de fichier dans le journal d'audit (`sftp_transfer` : sens seulement,
  `sftp_remove` : option récursive seulement).
- **Écritures atomiques** : `<fichier>.part` puis renommage ; reprise seulement si les 64 Kio
  de queue concordent (source modifiée / `.part` corrompu → redémarrage à zéro). Annuler
  supprime le `.part` du fichier en cours.
- **Liens symboliques** : ignorés par défaut dans les transferts récursifs ; « suivre » est
  opt-in, profondeur bornée (64) contre les boucles ; `chmod` récursif et `rm` récursif ne
  suivent jamais un lien.
- **Panneau local** : `local_list` expose à la webview un listing du système de fichiers de
  l'utilisateur (lecture seule, sans lecture de contenu) ; la CSP et les capabilities Tauri
  restent inchangées (aucun plugin `fs`).
- **OSC 7** : le répertoire annoncé par le serveur est une donnée non fiable : il ne fait que
  naviguer le panneau (jamais exécuté).
- **Injection du hook de suivi (opt-in)** : la case « Suivre le terminal » du panneau SFTP (cocher = action explicite) ou le réglage
  `sftp_follow_terminal` (désactivé par défaut) TAPENT une ligne fixe, compilée dans l'application
  (jamais construite depuis une donnée distante ou utilisateur), dans le shell interactif de la
  session — équivalent à la saisir soi-même. L'écho est masqué localement (SGR « conceal » puis effacement) : c'est un confort d'affichage, PAS une protection (le serveur voit la ligne, qui ne contient aucun secret). Aucun fichier distant modifié ; action auditée
  (`sftp_follow_enable`). Risque assumé : si un programme plein écran est au premier plan, la
  ligne lui est envoyée (d'où la confirmation « au prompt ») ; avec l'option automatique, elle
  est envoyée ~600 ms après la connexion. Alternative sans injection : copier le snippet.
- **Limite** : l'intégrité d'un transfert est vérifiée par la taille (SFTP v3 sans hash
  standard) ; la confidentialité/intégrité réseau est celle du transport SSH.

### 7.2 Édition à distance (M6)

- **Contenu en clair pendant l'édition** : le fichier distant est téléchargé dans
  `$XDG_RUNTIME_DIR/harakats-edit-<id>/<uuid>/` (**tmpfs**, jamais sur le disque ; repli sur
  `<données>/tmp/` sans runtime dir), répertoire **0700**, fichier **0600**. À la fermeture du document,
  au verrouillage du coffre et au démarrage (crash), les fichiers sont **écrasés par des zéros puis
  supprimés** (au mieux, sans garantie sur SSD/journalisation). Un éditeur externe peut en
  créer des copies (sauvegardes, swap) hors de notre contrôle : à éviter pour les fichiers sensibles.
- **Pas d'écrasement silencieux** : l'état distant (mtime, taille, SHA-256) est comparé avant chaque
  envoi ; tout changement concurrent est un conflit explicite (écraser / recharger / comparer).
- **Éditeur externe lancé sans shell** : la commande des réglages est découpée en arguments et
  exécutée directement (`Command`), jamais interprétée par un shell ; aucun plugin shell Tauri, aucune
  capability supplémentaire. Le réglage est celui de l'utilisateur (coffre chiffré) ; `{file}` est le seul
  paramètre substitué.
- **Audit** : `edit_open` (session), `edit_save` / `edit_conflict` / `edit_external` — jamais de chemin
  ni de contenu.
- **Surface réduite** : le retrait du moteur xterm a supprimé la capability de création de webview et les
  fenêtres `detach-*` ; la liste des commandes IPC est plus courte.

### 7.3 Tunnels, multi-exec, macros, journaux, imports (M7)

- **Tunnels** : bind **loopback par défaut** ; une adresse non loopback est signalée mais autorisée (choix
  explicite de l'utilisateur) ; SOCKS5 sans authentification donc à ne JAMAIS exposer hors loopback ;
  plafonds (256 connexions, délais) contre l'épuisement de ressources ; aucune charge utile journalisée ;
  audit `tunnel_start`/`tunnel_stop` sans destination.
- **Multi-exec** : action destructive par nature (une commande part dans N sessions) → confirmation à
  l'activation, indicateur permanent natif + bandeau, désactivation automatique sous 2 membres.
- **Macros** : contenu chiffré dans le coffre (champ secret), montré uniquement sur action explicite auditée ;
  envoi avec aperçu exact (anti-pastejacking) quand « confirmer » est coché ; le snippet de démarrage ne
  s'exécute jamais s'il exige une confirmation.
- **Journaux de session** (opt-in par connexion) : **chiffrés par défaut** (`.log.enc`, XChaCha20-Poly1305
  par blocs authentifiés, sous-clé SHA-256 dérivée de la DEK ; lisibles dans Paramètres coffre déverrouillé ;
  case « Chiffrer » décochable → `.log` clair), **rétention** configurable (30 jours par défaut, purge à
  chaque déverrouillage + bouton), fichiers 0600 dans un dossier 0700, séquences d'échappement retirées,
  plafond 50 Mio ; **limite assumée** : un mot de passe affiché en écho peut y figurer (avertissement dans
  l'éditeur) ; les charges OSC 52 (presse-papier) n'y entrent jamais.
- **Imports** : aperçu et clés restent dans le backend ; lecture de clés bornée (64 Ko, en-tête PRIVATE KEY) ;
  entrées hostiles bornées (4 Mio, 16 Ko/ligne, profondeur Include 8) ; jamais de modification de l'existant ;
  mots de passe Remmina/MobaXterm **non déchiffrés** (ressaisie) — pas de déchiffrement de secrets tiers.

### 7.4 Intégrité du coffre (durcissement)

- Écriture atomique (tmp 0600 → fsync → rename → **fsync du dossier parent**) puis **relecture de contrôle**.
- Rotation des 5 sauvegardes **uniquement si le coffre courant est structurellement valide** : un fichier
  corrompu ne peut jamais chasser une bonne sauvegarde.
- Au déverrouillage, un coffre illisible déclenche l'affichage des sauvegardes (marquées valides/corrompues) ;
  la restauration d'une sauvegarde corrompue est refusée.

## 8. Limites assumées (honnêteté)

1. **Poste compromis session ouverte (T2)** : un malware tournant sous l'utilisateur
   peut lire la mémoire du process, intercepter l'IPC, capturer le clavier. Aucun
   gestionnaire de coffre ne résout ce cas ; l'auto-lock réduit la fenêtre.
2. **Historique de presse-papier** : la copie d'un secret porte l'indicateur « secret »
   (`x-kde-passwordManagerHint`, comme les gestionnaires de mots de passe) que Klipper respecte :
   la valeur n'entre pas dans l'historique et n'est pas restaurée après l'effacement. Les gestionnaires
   qui ignorent cet indicateur peuvent encore conserver la valeur. Le presse-papier est effacé
   aussitôt au verrouillage du coffre (si le contenu est toujours le dernier secret copié), sinon à
   l'expiration du délai. L'UI rappelle la limite lors d'un « copier un secret ».
3. **Journalisation de session** : la sortie terminal peut contenir des secrets echoés ;
   c'est inhérent à un terminal. Opt-in, averti, 0600.
4. **Core dumps / swap** : la DEK peut se retrouver dans un dump ou le swap si l'OS les
   autorise. Recommandations documentées (swap chiffré, `ulimit -c 0`) ; l'app ne
   désactive pas les dumps système (hors de son périmètre, poste de l'utilisateur).
5. **Metadata réseau** : les connexions SSH révèlent IPs/ports/hôtes à l'observateur
   réseau (le coffre chiffre le repos, pas le trafic).
6. **Passphrase d'export faible** : la sécurité du bundle = force de la passphrase
   d'export + Argon2id. L'UI affiche un indicateur de force et refuse les passphrases
   vides ; pas de longueur minimale arbitraire imposée au-delà de 8 caractères
   (avertissement en dessous).
7. **Pas de protection contre un attaquant root** : root peut tout (mémoire, disque,
   clavier). Hors modèle.
8. **RNG** : on fait confiance à `getrandom`/OsRng (noyau). Pas de HSM/TPM en v1.
9. **Mot de passe maître oublié = données perdues** : aucune porte dérobée, aucun
   « recovery key » stocké ailleurs. C'est le contrat. (Piste post-v1 : phrase de
   récupération optionnelle à imprimer, wrap supplémentaire de la DEK.)

## 9. Journal d'audit (local, sans secrets)

`audit.log` (JSON lines, 0600, rotation) : horodatage, événement, identifiant de cible,
résultat. Événements : `vault_unlock` (méthode master/keyring), `vault_lock` (raison),
`master_change`, `keyring_cache_on/off`, `credential_reveal`, `clipboard_copy_secret`,
`export` (scope, includes_secrets), `import` (source, stratégie, compteurs),
`hostkey_tofu_accept`, `hostkey_change_rejected/replaced`, `session_log_enabled`,
`external_editor_launch`. consultable dans l'UI (réglages → audit).

## 10. Signalement de vulnérabilité

Projet personnel en construction : pas de programme de bug bounty. À terme, un contact
sécurité sera publié dans ce fichier (placeholder : `security@<domaine du projet>` —
**ne pas publier d'adresse personnelle**). Politique de divulgation coordonnée 90 jours
envisagée pour la v1 publique.
