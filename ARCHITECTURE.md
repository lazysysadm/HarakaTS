# ARCHITECTURE — HarakaTS v1

> Phase 0 — document de conception. À valider avant écriture de code.
> Versions et APIs vérifiées le 2026-11-20 (sources en fin de document).

## 1. Objectifs

Gestionnaire de connexions distantes Linux (SSH + SFTP uniquement en v1) combinant :

- **Arbre de connexions** : arbre de dossiers, héritage de propriétés, identifiants découplés ;
- **MobaXterm** : terminal à onglets, panneau SFTP lié à la session, multi-exec, tunnels ;
- **Remmina** : imports de configurations — **sans son défaut majeur** : ici les secrets
  sont chiffrés dans un coffre portable, indépendants du keyring de la machine.

**Critère d'acceptation n°1** : export sur profil A → import sur profil B vierge (autre
répertoire de données, autre mot de passe maître) → toutes les connexions s'ouvrent avec
leurs identifiants. Test automatisé en CI à chaque jalon.

## 2. Choix techniques et justifications

### 2.1 Frontend : Svelte 5 (runes) — choix justifié

| Critère | Svelte 5 | React 19 |
|---|---|---|
| Poids runtime | ~2–5 Ko, compilé | ~45 Ko + écosystème |
| Mises à jour haute fréquence (terminal, transferts) | réactivité fine par signal, pas de VDOM | re-renders + mémoïsation manuelle |
| Intégration widgets impératifs (CodeMirror ; terminal VTE natif hors webview) | simple : effets + runes, pas de cycle de vie composite | possible mais plus de boilerplate |
| TypeScript strict | support natif | natif |

Une app de terminaux passe son temps à pousser des octets dans des widgets impératifs
(CodeMirror ; le terminal, lui, est natif GTK+VTE hors webview depuis M3-VTE) : le
compilateur Svelte et sa réactivité granulaire collent mieux à ce profil qu'un VDOM.
**Décision : Svelte 5 + Vite + TypeScript strict.**

### 2.2 Persistance : fichiers JSON versionnés chiffrés — choix justifié

**Décision : un fichier-coffre unique** (`vault.hrk`) contenant un store JSON versionné
chiffré, écritures atomiques (tmp + rename), sauvegardes rotatives. Pas de SQLite en v1.

- La portabilité est le critère n°1 : un fichier unique auto-descriptif (en-tête + payload
  chiffré) s'exporte, se copie, se versionne trivialement. SQLite ajouterait un moteur,
  des fichiers WAL/journal et une complexité de fusion sans bénéfice à l'échelle v1
  (quelques centaines de connexions, pas de requêtes relationnelles).
- Le format interne est du JSON canonique (`serde_json`) : auditable, diffable après
  déchiffrement, schéma migrable par `schema_version`.
- Si le volume grossit (historique de sessions, milliers d'entrées), SQLite pourra être
  introduit derrière l'interface `Store` sans changer le format d'export.

### 2.3 keyring 3.6.2 (et non 4.x)

La v4 de `keyring` réorganise l'API (`keyring-core` + crates de stores). La v3.6.2 est
stable, documentée, et suffit : `Entry::new(service, user)`, `set_secret`/`get_secret`,
features `sync-secret-service` + `crypto-rust` + `vendored` (libdbus liée statiquement →
les paquets .deb/.rpm n'exigent pas libdbus chez l'utilisateur). Migration v4 possible post-v1.
**Exigence utilisateur : sans KWallet/Secret Service, l'app fonctionne — le cache keyring
est désactivé silencieusement, le mot de passe maître reste la seule voie de déverrouillage.**

### 2.4 Alignement rand_core (piège évité)

`chacha20poly1305 0.11` → `aead 0.6` → **rand_core 0.10** ; `rand 0.10.4` utilise aussi
rand_core 0.10. **rand 0.10 a supprimé `OsRng`** au profit de `SysRng` (feature
`sys_rng`, trait `TryRng::try_fill_bytes`) — même source système, API faillible. Tout
l'aléatoire cryptographique (DEK, sels, nonces) vient de `SysRng` ; les nonces sont
construits par `XNonce::from_slice` (24 o) sans passer par le trait `Generate` d'aead,
ce qui évite toute question de bound de trait. `argon2 0.6` s'utilise en API brute
(`hash_password_into`, le sel est fourni par nous). **Aucun mélange rand_core
0.6/0.9/0.10** — documenté pour ne pas y retomber lors des mises à jour (les anciennes
versions argon2 0.5 / aead 0.5 / rand 0.8 créaient deux lignées incompatibles).

### 2.5 Versions épinglées (vérifiées le 2026-11-21 contre Cargo.lock / ui/package.json)

**Rust** (MSRV : 1.90, édition 2024, workspace resolver 2) :

| Crate | Version | Rôle |
|---|---|---|
| tauri | 2.12.x | shell applicatif, IPC, bundling deb/rpm |
| russh | 0.65.2 | client SSH (tokio) |
| russh-sftp | 3.0.1 | client SFTP v3 sur canal russh |
| argon2 | 0.6.0 | KDF Argon2id (API brute `hash_password_into`) |
| chacha20poly1305 | 0.11.0 | XChaCha20-Poly1305 (aead 0.6) |
| rand | 0.10.4 | SysRng (successeur d'OsRng, rand_core 0.10) |
| secrecy | 0.10.3 | `SecretString`/`SecretBox` (Debug rédigé) |
| zeroize | 1.9.2 | effacement mémoire |
| keyring | 3.6.3 | cache Secret Service (features ci-dessus) |
| tokio | 1.x | runtime async |
| serde / serde_json | 1.x | store JSON versionné |
| uuid | 1.x (v4) | identifiants |
| thiserror | 2.x | erreurs typées |
| tracing / tracing-subscriber | 0.1 / 0.3 | logs backend (secrets rédigés) |
| zbus | 6.4.0 | signaux logind / ScreenSaver (auto-lock) |
| arboard | 3.7.0 | presse-papier + sélection primaire Linux |
| notify | 8.2.2 | surveillance fichier local (édition distante M6) |
| encoding_rs | 0.8.x | encodages terminal non-UTF-8 |
| sha2 / hmac / sha1 | 0.10 / 0.12 / 0.10 | empreintes host keys, known_hosts hashés, intégrité |
| base64 | 0.22 | encodages en-tête/format |
| rfd | 0.18.0 | dialogues fichier natifs côté backend (backend **xdg-portal** + wayland, sync via pollster ; surtout pas la feature `gtk3` : gtk-sys 0.19 entrerait en conflit `links = "gtk-3"` avec le gtk-rs 0.18 de Tauri) |
| gtk | 0.18.2 | fenêtres terminal natives VTE (même lignée gtk-rs que Tauri, `links = "gtk-3"` unifié) |
| libvte-2.91 (FFI) | ≥ 0.50 (hôte Fedora 45 : 0.84.1 ; bookworm : 0.70) | moteur terminal natif façon Remmina ; probe pkg-config dans build.rs ; runtime RPM `vte291`, DEB `libvte-2.91-0` |

**Frontend** (Node 22 LTS, pnpm via corepack) :

| Paquet | Version |
|---|---|
| svelte | 5.58.0 |
| @codemirror/* (state, view, commands, language, language-data, search, merge) | 6.x épinglé (view 6.43.15, state 6.7.6, merge 6.12.3) — édition distante M6, chargée à la demande |
| vite | 8.3.8 |
| @sveltejs/vite-plugin-svelte | 7.3.1 |
| typescript | ~5.9 (strict) |
| svelte-check | 4.7.8 |
| eslint / typescript-eslint | ^9 / ^8 |
| vitest | 5.1.1 |
| @tauri-apps/api / @tauri-apps/cli | 2.12.5 / 2.12.6 |
| fuzzysort | 4.0.2 — recherche floue Ctrl+K |

**Terminal : xterm.js retiré en M6** (demande utilisateur) — VTE natif est le
**moteur unique** (§8) ; les paquets `@xterm/*` ne font plus partie du projet
(bundle UI 580 → 316 Ko). OSC 52 : interception **backend** (§9), aucun handler
côté renderer. Historique des pièges xterm/WebKitGTK : mémoire projet
`xtermjs_webkitgtk_gel` (ne pas réintroduire xterm).

## 3. Structure du workspace

```
HarakaTS/
├── Cargo.toml                  # workspace
├── crates/
│   ├── haraka-core/            # AUCUNE dépendance Tauri/GUI → testable headless en CI
│   │   ├── src/vault/          # coffre, DEK/KEK, Argon2id, AEAD, AAD, calibration KDF
│   │   ├── src/model/          # Folder, Connection, Credential, Macro, Settings, héritage
│   │   ├── src/store/          # lecture/écriture atomique, backups, migrations schéma
│   │   ├── src/export/         # bundle export/import, fusion UUID, stratégies conflit
│   │   ├── src/importers/      # ssh_config, remmina, mobaxterm
│   │   ├── src/knownhosts/     # known_hosts interne + import système (entrées hashées)
│   │   └── src/audit/          # journal d'audit local sans secrets
│   └── haraka-ssh/             # backend de session SSH (dépend de haraka-core)
│       ├── src/session/        # SessionBackend SSH, auth, host keys, jump hosts
│       ├── src/tunnels/        # L / R / D (SOCKS5 minimal)
│       ├── src/sftp/           # navigateur + moteur de transferts
│       └── src/edit/           # édition distante (temp, watch, conflits, upload atomique)
├── src-tauri/                  # couche IPC mince : commandes, événements, capabilities
├── ui/                         # Svelte 5 + Vite + TS strict
│   └── src/lib/{tree,terminal,sftp,editor,dialogs,palette,state,i18n}/
├── tests/                      # fixtures imports, serveur SSH de test, scénarios CI
├── packaging/                  # Containerfile dev, configs bundling, CI
└── docs/                       # (les .md racine font foi ; docs/ = annexes futures)
```

Règle de dépendances : `ui → src-tauri → haraka-ssh → haraka-core`. `haraka-core` ne
connaît ni Tauri ni russh : le test d'export/import A→B tourne sans GUI ni réseau.

## 4. Modèle de données

Tout le store vit **dans le payload chiffré du coffre** (voir SECURITY.md §2 pour la
double couche de chiffrement). Schéma interne `schema_version: 1`.

```jsonc
{
  "schema_version": 1,
  "vault_id": "uuid-v4",
  "created_at": "RFC3339", "updated_at": "RFC3339",
  "settings": {
    "autolock_minutes": 15,            // 0 = désactivé
    "lock_on_suspend": true, "lock_on_session_lock": true,
    "disconnect_on_lock": true,
    "keyring_cache": false,            // opt-in, jamais requis (défaut: désactivé)
    "clipboard_default": "bidirectional",
    "osc52_max_bytes": 100000,
    "paste_confirm_multiline": true,
    "copy_on_select": false,
    "clipboard_clear_seconds": 30,
    "kdf_target_ms": [500, 1000],
    "locale": "fr", "theme": "system",
    "external_editor": null,           // commande configurable, exécution backend
    "transfer_parallelism": 3,
    "reconnect": { "enabled": true, "max_attempts": 5, "backoff_max_s": 30 }
  },
  "folders": [{
    "id": "uuid", "parent_id": "uuid|null", "name": "Prod",
    "color": "#rrggbb", "icon": "server", "sort_order": 0, "tags": ["prod"],
    "defaults": {                      // héritage (voir §4.1)
      "user": "deploy", "port": 22, "credential_id": "uuid|null",
      "jump_chain": [], "clipboard_policy": null, "keepalive_s": 30,
      "encoding": "utf-8", "auth_method_order": ["agent","publickey","password","keyboard-interactive"]
    }
  }],
  "connections": [{
    "id": "uuid", "folder_id": "uuid", "name": "web-01",
    "protocol": "ssh",                 // seul protocole v1 ; extensible
    "host": "web-01.example.net", "port": null, "user": null,   // null = hérite
    "credential_id": null,             // null = hérite du dossier
    "jump_chain": [ { "host": "bastion", "port": 22, "user": "jump", "credential_id": "uuid" } ],
    "clipboard_policy": null,          // null = hérite ; sinon disabled|l2r|r2l|bidirectional
    "keepalive_s": null, "encoding": null,
    "color": null, "icon": null, "tags": [], "favorite": false,
    "tunnels": [ { "id":"uuid", "kind":"local|remote|dynamic",
                   "bind_addr":"127.0.0.1", "bind_port":8080,
                   "dest_host":"db.internal", "dest_port":5432, "auto_start":true } ],
    "startup_snippet_id": null,
    "session_logging": false,          // opt-in par connexion (voir SECURITY.md §7)
    "sftp": { "follow_terminal_cwd": false, "show_hidden": false },
    "notes": "", "created_at": "…", "updated_at": "…", "last_used_at": null
  }],
  "credentials": [{
    "id": "uuid", "name": "deploy-prod",
    "username": "deploy",              // non secret (métadonnée)
    "password":   { "alg":"XChaCha20-Poly1305", "nonce":"b64", "ct":"b64" },  // EncSecret|null
    "private_key": { … },              // contenu PEM/OpenSSH complet, chiffré
    "passphrase":  { … },              // passphrase de la clé, chiffrée
    "key_origin": "imported|generated|pasted",
    "needs_reentry": ["password"],     // champs marqués « à ressaisir » (imports)
    "created_at": "…", "updated_at": "…"
  }],
  "macros": [{ "id":"uuid", "name":"restart-nginx",
               "content": { …EncSecret… },   // chiffré : un snippet peut contenir un secret
               "confirm_before_send": true }],
  "recents": [ { "connection_id":"uuid", "last_used_at":"…" } ],   // borné à 50
  "ui_state": { "expanded_folders": ["uuid"], "tab_layout": { … } }
}
```

`EncSecret` : nonce 24 o aléatoire + ciphertext XChaCha20-Poly1305 (tag 16 o inclus),
**AAD = vault_id ∥ entry_id ∥ nom_du_champ** (encodage canonique à préfixes de longueur,
voir EXPORT_FORMAT.md §4). Un secret ne peut donc pas être déplacé entre entrées/champs.

### 4.1 Héritage des propriétés

Résolution **purement backend**, au moment de la connexion :

```
config_effective = fold(defaults_système, defaults_dossier_racine → … → dossier_parent, overrides_connexion)
```

- Un champ `null` sur la connexion = hérite ; une valeur = surcharge.
- Les dossiers forment une chaîne `parent_id` (profondeur libre, cycles interdits —
  vérifiés à l'écriture).
- `credential_id` se résout de la même façon ; l'objet Credential est partagé : le
  modifier une fois met à jour toutes les connexions qui le référencent.
- La politique presse-papier est héritable dossier → connexion (exigence §3 du prompt).
- L'UI affiche la valeur effective avec un badge « hérité de <dossier> » et un bouton
  « surcharger ».

### 4.2 Références et intégrité

- Toutes les références sont des UUID ; suppression d'un credential référencé →
  avertissement + liste des connexions impactées, au choix : détacher (les connexions
  retombent sur l'héritage) ou annuler.
- `needs_reentry` : une connexion dont le credential a des champs manquants s'ouvre en
  mode « authentification à compléter » (invite mot de passe/passphrase), sans jamais
  planter ni bloquer l'import.

## 5. Coffre et crypto (résumé — détail dans SECURITY.md et EXPORT_FORMAT.md)

- **DEK** 256 bits aléatoire (SysRng) → chiffre le payload du store **et** chaque secret
  individuellement (double couche, voir SECURITY.md §2).
- **KEK** = Argon2id(mot de passe maître, sel, m, t, p) → wrap de la DEK dans l'en-tête.
- Paramètres KDF **calibrés à la création** pour 0,5–1 s sur la machine (m = 128 MiB,
  p = 4, t ajusté ; stockés dans l'en-tête, re-calibrables dans les réglages).
- Changement de mot de passe maître = **re-wrap de la DEK uniquement** (nouveau sel),
  le payload n'est pas re-chiffré.
- Fichier `vault.hrk` : enveloppe binaire versionnée — en-tête JSON immuable + bloc
  KDF/DEK wrappée (mutable, authentifié par le tag du wrap) + payload AEAD. Le
  changement de master ne réécrit que le bloc KDF/wrap (payload inchangé, O(1)).
  Format binaire détaillé : EXPORT_FORMAT.md (amendement du 2026-11-20).
- Écritures atomiques : `vault.hrk.tmp` (0600) → fsync → rename ; `backups/` conserve les
  5 dernières versions ; payload corrompu → proposition de restauration d'un backup.
- **Keyring OS (optionnel, non bloquant)** : si `keyring_cache = true` et qu'un Secret
  Service répond (KWallet sous KDE), la DEK y est déposée (`service="harakats"`,
  `user=vault_id`, `set_secret`) pour déverrouiller sans mot de passe sur ce poste.
  Échec/absence du service → repli silencieux sur le mot de passe maître. La DEK wrappée
  par le mot de passe maître reste **toujours** dans l'en-tête : le keyring n'est jamais
  indispensable (c'est la différence fondamentale avec Remmina).
- **Auto-lock** : timer d'inactivité (événements UI → backend), signal logind
  `PrepareForSleep(true)`, verrouillage de session (`org.freedesktop.ScreenSaver` /
  session logind, via zbus). Au verrouillage : `zeroize` DEK + clés en mémoire,
  déconnexion des sessions (défaut, configurable), l'UI repasse à l'écran de
  déverrouillage.

## 6. Abstraction protocoles

```rust
#[async_trait]
pub trait SessionBackend: Send + Sync {
    fn protocol(&self) -> Protocol;                    // Ssh (v1) ; Rdp, Vnc, Telnet, Serial ensuite
    fn capabilities(&self) -> Capabilities;            // terminal, filesystem, tunnels, clipboard…
    async fn connect(&mut self, cfg: &EffectiveConfig, secrets: &SecretResolver) -> Result<()>;
    async fn open_terminal(&mut self, size: TermSize) -> Result<TerminalHandle>;
    async fn open_filesystem(&mut self) -> Result<FsHandle>;      // SFTP pour SSH
    async fn open_tunnel(&mut self, spec: &TunnelSpec) -> Result<TunnelHandle>;
    async fn disconnect(&mut self) -> Result<()>;
    fn state(&self) -> SessionState;                   // Connecting, AuthPrompt, Connected, Reconnecting, Closed
}
```

- `TerminalHandle` : `write(bytes)`, `resize(size)`, `close()`, flux de sortie via
  `tokio::mpsc` (borné, coalescé).
- `FsHandle` : interface type `std::fs` async (list, stat, read, write, rename, remove,
  chmod/chown, mkdir, symlink) — implémentée par russh-sftp.
- `SecretResolver` : accès aux secrets **côté backend uniquement**, le temps de l'auth ;
  les matériaux sont zeroizés après usage.
- **Honnêteté v1** : RDP/VNC auront une surface de rendu différente (image/vidéo, pas un
  terminal texte). L'abstraction couvre cycle de vie, auth, presse-papier et tunnels ;
  `capabilities()` permettra à l'UI de choisir le widget. Ajouter un protocole ne
  refactorera pas le coffre, l'arbre, l'IPC ni les transferts — mais un backend RDP
  demandera son propre module de rendu (hors v1).
- `Protocol` est un enum sérialisé ; une connexion inconnue au schéma → erreur propre,
  jamais de panic.

## 7. Module SSH (`haraka-ssh`)

APIs russh 0.65.2 **vérifiées** (docs.rs) :

- `client::connect(config, addr, handler)` / `client::connect_stream(config, stream, handler)`
  (stream = `AsyncRead + AsyncWrite + Unpin + Send`) → base des jump hosts.
- Auth : `Handle::authenticate_password`, `authenticate_publickey(user, PrivateKeyWithHashAlg)`
  (RSA : `best_supported_rsa_hash` pour choisir l'alg de hash), et keyboard-interactive
  en **deux étapes** : `authenticate_keyboard_interactive_start(user, submethods)` puis
  `authenticate_keyboard_interactive_respond(Vec<String>)` (boucle tant que le serveur
  renvoie des prompts — 2FA/OTP).
- Clés : `russh::keys` (chargement/déchiffrement de clés ed25519/ecdsa/rsa, **agent SSH**
  via `SSH_AUTH_SOCK`). Symboles exacts (`load_secret_key`, `agent::AgentClient`,
  `known_hosts`) re-vérifiés au M3 ; repli : implémentation minimale du protocole agent
  (il est simple et stable) si l'API a bougé.
- Host keys : `Handler::check_server_key(&PublicKeyOrCertificate)` — l'impl par défaut
  **rejette tout**, elle est surchargée :
  1. empreinte SHA256 (format OpenSSH `SHA256:base64-sans-padding`) ;
  2. known_hosts interne (`data/known_hosts`, format OpenSSH, 0600) + import du
     `~/.ssh/known_hosts` système (entrées hashées `|1|sel|hash` supportées : HMAC-SHA1) ;
  3. inconnue → **TOFU** : dialogue bloquant avec empreinte, « Accepter et mémoriser » /
     « Refuser » ;
  4. **clé changée → alerte bloquante rouge**, anciennes/nouvelles empreintes affichées,
     refus par défaut ; remplacement seulement sur action explicite, journalisé (audit).
  Le handler attend la réponse UI via un `oneshot` — aucune décision automatique.
- Canaux : `channel_open_session()` → `request_pty(term, cols, rows, …)` →
  `request_shell(true)` ; resize = **`window_change`** (pas `resize_pty`, qui n'existe
  pas) ; écriture = `data_bytes` / `make_writer` ; sortie = `Handler::data` → mpsc.
  **Règle russh impérative** : russh envoie chaque message du canal à `Handler::data` ET à
  la file interne du `Channel` (bornée, `channel_buffer_size` = 100). Cette file doit être
  **drainée** : le canal shell est `split()` ; la moitié lecture est consommée par une tâche
  (`read_half.wait()`), la moitié écriture (`ChannelWriteHalf`) sert à `data_bytes` /
  `window_change`. Sans drain, la boucle de session se bloque au ~100e message (gel htop).
  Verrou `inner` jamais tenu pendant un `await` réseau (handle cloné puis relâché).
- **Jump hosts chaînés** : TCP → russh(hop1) → auth → `channel_open_direct_tcpip(hop2)`
  → `into_stream()` → `connect_stream`(hop2) → … → cible. Chaque hop a son credential
  (héritage inclus). `ProxyJump` importé de ssh_config mappe directement cette chaîne.
  `ProxyCommand` : **reporté post-v1** (exige un spawn shell, surface de risque inutile).
- **Tunnels** :
  - *Local (L)* : listener tokio sur `bind_addr:port` → par connexion entrante
    `channel_open_direct_tcpip(dest)` → pompe bidirectionnelle ;
  - *Remote (R)* : `Handle::tcpip_forward(addr, port)` (port 0 = choisi par le serveur,
    renvoyé) → `Handler::server_channel_open_forwarded_tcpip` → connexion locale vers la
    cible → pompe ; dépend de `GatewayPorts` côté serveur (documenté dans l'UI) ;
  - *Dynamique (D)* : serveur **SOCKS5 minimal** (RFC 1928 : no-auth + CONNECT,
    IPv4/IPv6/domaine ; pas d'UDP ASSOCIATE ni BIND — documenté) → `direct-tcpip`.
  - Gestionnaire graphique : specs par connexion + tunnels live (ajout/retrait à chaud,
    état, compteurs, auto_start).
  - *Implémentation M7* (`haraka-ssh::tunnel`, `src-tauri/src/tunnels.rs`) : `TunnelManager` par session
    (états `starting|listening|error`, compteurs rafraîchis ≤ 1/s, événement `tunnel://state`), registre
    des forwards distants partagé avec `SshHandler`, démarrage auto idempotent via `post_connect.rs`,
    arrêt à la fermeture de session. Multi-exec : `haraka-ssh::multiexec` (`write_routed` écrit à
    l'origine puis aux autres membres via `SessionManager::write`, jamais par leur file d'entrée → pas de
    boucle). Journal de session : `haraka-core::sessionlog` branché sur le forwarder de sortie après OSC 52.
- Keepalive : tâche `send_keepalive(want_reply=false)` à intervalle configurable ;
  `send_ping` pour mesurer la latence (affichée en barre d'état).
- Reconnexion auto : backoff 1→2→4→…→30 s, max 5 tentatives (réglable), ré-auth depuis
  le cache mémoire backend (uniquement coffre déverrouillé) ; échec final → onglet en
  état « déconnecté », bouton reconnecter. Au verrouillage du coffre : pas de reconnexion.
- Encodage : UTF-8 par défaut, `encoding_rs` pour les charset legacy configurables par
  connexion (héritable).
- Agent forwarding : **désactivé, non proposé en v1** (risque d'abus du socket agent par
  le serveur distant ; décision de sécurité documentée dans SECURITY.md).

## 8. Terminal et UI

- **Deux moteurs (M3-VTE, décision utilisateur du 2026-11-20)** :
  - **VTE natif (défaut, `settings.terminal_engine = "vte"`)** — le moteur de Remmina.
    Fenêtre GTK+VTE **par session** (widget natif, rendu/clavier/resize/copier-coller
    système). FFI direct sur **libvte-2.91** (API C stable, LGPL) : les bindings Rust
    existants (`vte-rs`) sont GPL-3.0 ou basés sur un GTK incompatible avec Tauri.
    Mode « feed » sans PTY local : sortie SSH décodée → `vte_terminal_feed` ; clavier →
    signal « commit » ; « size-allocate » → `window_change`. Garanties (revue
    ameliorations.md, 2026-11-20) : dimensions lues en **idle** (grille VTE à jour,
    jamais l'ancienne taille) ; **une tâche ordonnée par session** (mpsc Input/Resize,
    encodeur local — pas de réordonnancement saisie/resize) ; write/resize russh
    **hors du verrou global** (Arc<Channel> cloné sous verrou, await réseau dehors) ;
    sortie **coalescée** (un seul invoke GTK en vol, tampon borné 16 Mo) ; resize
    débouncé 100 ms. Barre d'état native (état/latence/dims/Reconnecter), menu
    contextuel (copier/coller VTE natifs, sélection, reset), erreurs visibles dans
    le terminal. Toutes les calls GTK sur le thread principal (glib MainContext),
    jamais depuis tokio. Prompts auth/host keys = dialogues GTK natifs (même
    politique TOFU ; Annuler = réponse vide → rejet propre).
    Motif du pivot : xterm.js dans WebKitGTK figeait htop/tmux (renderer WebGL instable
    + inondation IPC) malgré le renderer DOM et le batching — le backend était sain,
    le rendu webview en cause. VTE sort le terminal de la webview.
  - **Moteur unique** : l'ancien repli xterm.js (onglets, splits, détachement) a été retiré (M6) ;
    la capability Tauri `core:webview:allow-create-webview-window` et la fenêtre `detach-*` avec lui.
- Sessions : une fenêtre GTK+VTE par session (le backend garde l'état de session), **ou** un onglet
  d'un `gtk::Notebook` placé au-dessus de la webview dans la fenêtre principale (option
  `terminal_in_tabs`, module `vte/tabs.rs` : la webview est replacée dans un `gtk::Paned` à la première
  ouverture d'onglet ; `VteWindow.page_ptr != 0` = mode onglet, fermeture via `handle::close_tab`).
  Barre de sessions dans la fenêtre principale (store `sessions.svelte.ts`, poll 2 s + événements
  `session://opened|closed|active` émis par `sessions.rs` / focus GTK) ; le panneau SFTP suit la session
  active.
- **Coloration de la sortie** : `haraka-core::highlight::Highlighter` (règles fixes, machine à états
  qui saute les séquences d'échappement, suit l'écran alternatif et la couleur de premier plan posée
  par le serveur), appliquée dans `spawn_output_forwarder` APRÈS la journalisation (journal brut).
  Réglage `terminal_highlight`.
- Flux de données : `Handler::data` → mpsc borné → **`tauri::ipc::Channel<Vec<u8>>`**
  (sérialisation `serde_bytes`, pas de base64) → `term.write` batché par
  `requestAnimationFrame`. Entrée : `session_write(session_id, bytes)` (invoke).
  Backpressure : coalescence des petits paquets, canal borné (le SSH flow-control fait
  le reste).
- **Multi-exec** (panneau flottant non modal côté UI) : sélection d'onglets/panneaux → chaque frappe est dupliquée vers les
  sessions sélectionnées (fan-out backend), indicateur visuel permanent « MULTI-EXEC »
  + confirmation à l'activation.
- Macros/snippets : contenu chiffré dans le coffre, exécution = envoi d'octets aux
  sessions choisies, confirmation configurable. Pas d'interpolation de secrets dans les
  macros en v1 (éviter l'écho involontaire ; documenté).
- Barre de connexion rapide `user@host:port` (création à la volée, proposition de
  sauvegarde), favoris, récentes, **recherche floue Ctrl+K** (fuzzysort : connexions,
  dossiers, credentials, actions).
- Journalisation de session **optionnelle par connexion** : flux terminal brut vers
  `data/logs/<session>.log` (0600). Avertissement UI : la sortie terminal peut contenir
  des secrets echoés (mots de passe tapés en clair côté shell) ; aucun matériau d'auth
  backend n'y est jamais écrit (voir SECURITY.md §7).
- i18n : catalogues JSON FR (défaut) + EN, structure prête dès M2.

## 9. Presse-papier (politiques par connexion, héritables)

Politique : `disabled | local→remote | remote→local | bidirectional` (défaut système
`bidirectional`, surcharge dossier puis connexion).

| Sens | Mécanisme | Contrôle |
|---|---|---|
| local → distant | collage terminal (Ctrl+Shift+V, clic milieu, collage bracketé) | confirmation multiligne/suspect (`paste_confirm_multiline`) ; action utilisateur explicite |
| distant → local (write) | **OSC 52 intercepté dans le backend** (`haraka_core::osc52::Osc52Extractor`, pipeline de sortie — les séquences n'atteignent jamais le terminal) | politique `r2l`/`bidirectional` + `osc52_max_bytes` (défaut 100 000 o), audité |
| distant → local (query) | OSC 52 `?` : le distant **lit** le presse-papier local (exfiltration potentielle) | politique `l2r`/`bidirectional`, **toujours audité**, réponse via `query_response` |
| copie locale | Ctrl+Shift+C, **copy-on-select** (option) → presse-papier + sélection primaire (arboard, extensions Linux) | politique locale |
| fichiers | copier-coller entre panneau local et panneau SFTP distant (format interne `application/x-harakats-files`) | **M5** (dépend du panneau SFTP ; écart M4 documenté) |
| drag & drop OS → app | événements drag-drop Tauri v2 (chemins de fichiers) → upload SFTP | **M5** ; jamais de chargement d'URL distante |

Implémentation M4 (2026-11-20) : extracteur streaming avec carry inter-chunks
(séquences coupées entre paquets), terminateurs BEL et ST, autres OSC intactes ;
politique effective héritée capturée à l'ouverture (`ClipCtx` par session).
Confirmation de collage : `haraka_core::paste` (Rust, dialogue GTK avec aperçu
␛␍␊) — marqueurs bracketed paste retirés **en bordure uniquement** (marqueur
interne = pastejacking → confirmation). Depuis M6, le moteur est VTE unique :
le miroir TypeScript `paste.ts` et le chemin xterm ont été retirés ; le collage
VTE passe par `paste_from` (confirmation unique, jamais sur le signal `commit`).

- **Bracketed paste** toujours actif côté terminal distant (le shell distant décide).
- **Anti-collage malveillant** : avant tout collage, analyse du contenu ; confirmation
  si multiligne (réglable) ou si caractères de contrôle/échappements suspects (hors
  `\n`, `\t`), avec aperçu exact de ce qui sera envoyé.
- Copie d'un secret (action explicite) : `clipboard_copy_secret` → écriture presse-papier
  backend → **effacement automatique après N secondes** (défaut 30, réglable) ; le secret
  n'est jamais stocké côté frontend. Limite assumée : les gestionnaires d'historique de
  presse-papier (Klipipper…) peuvent conserver une trace — documenté dans SECURITY.md.
- **Code VTE** (`src-tauri/src/vte/`) : `ffi` (seul `unsafe`, wrapper `Term`), `handle` (poignée
  `Send+Sync` + registre, retour au thread GTK via `on_main_if_alive`), `window` (assemblage),
  `clipboard`, `resize`, `dialogs`, `mod` (`VteCallbacks`, `VteOptions`, `open_window`).
- **Collage VTE natif** : la confirmation n'est JAMAIS sur le signal `commit` (frappes, séquences
  de touches, marqueurs bracketed paste fragmentés). Le collage explicite est lu côté GTK
  (`paste_from`, CLIPBOARD ou PRIMARY), confirmé une seule fois si suspect, puis envoyé par
  `SessionCmd::Paste` (encadré `ESC[200~…ESC[201~` si le terminal a activé DECSET 2004, suivi
  dans le flux de sortie). Ctrl+Maj+C/V, Ctrl/Maj+Inser sont câblés par l'application.
- **Limite v1 assumée** : le drag-out (glisser un fichier distant vers le gestionnaire de
  fichiers OS) n'est pas supporté par les webviews Tauri v2 ; remplacé par « Copier
  vers… » / téléchargement. Le drag-in OS → app fonctionne.

## 10. SFTP et transferts

> **Statut M5 : implémenté** (`crates/haraka-ssh/src/{sftp,transfer,osc7}.rs`, `src-tauri/src/sftp.rs`,
> `ui/.../SftpPanel.svelte`). Points de conception effectifs :
> - `SessionManager::sftp(session)` : ouverture mono-vol d'un 2ᵉ canal (`Arc<Handle>` cloné hors
>   verrou global), mise en cache dans `SessionEntry`, `sftp_invalidate` sur erreur fatale.
> - **`SshHandler::data` n'alimente le terminal que pour le canal shell** (`shell_channel`) ;
>   sinon les octets SFTP seraient affichés. Le canal SFTP (`into_stream`) est lu en continu
>   par `SftpSession` : pas de file russh à drainer.
> - Reprise : `.part` + comparaison des 64 Kio de queue des deux côtés (pas de sidecar) ;
>   `rename` final (SFTP v3 n'écrase pas → suppression préalable si « remplacer »).
> - OpenSSH lit `SSH_FXP_SYMLINK` en (cible, lien) — `sftp::symlink` inverse les arguments
>   de russh-sftp.
> - Suivi du terminal : `sftp::FOLLOW_HOOK` (ligne fixe bash/zsh, idempotente) tapée via
>   `SessionManager::write` par `sftp_enable_follow` ou, si `settings.sftp_follow_terminal`, à chaque
>   état `connected` d'une session VTE (`forward_native`).
> - Masquage de la ligne du hook : `vte::begin_conceal` (SGR 8 + calcul du nombre de lignes d'écho)
>   → `ClipCtx.hook_rows` → le forwarder de sortie insère `ESC[0m ESC[nA CR ESC[J` juste après le premier
>   OSC 7 (`Osc7Scanner::first_end`) ; minuterie de garde 4 s.
> - SFTP : vue intégrée non modale du Dashboard (`SftpPanel` monté une fois, `hidden` bascule).
> - Fenêtre principale : `window_state.rs` (fenêtre créée invisible, taille restaurée puis `show`,
>   sauvegarde sur `CloseRequested`/`Destroyed`, taille « normale » suivie hors maximisé).
> - Événements UI : `transfer://event` (instantané `TransferInfo`, ≤ 5/s par job), `sftp://cwd`
>   (OSC 7), `sftp://open` (bouton de la fenêtre VTE).

- **Même connexion SSH** : `channel_open_session()` → `request_subsystem("sftp")` →
  `channel.into_stream()` → `russh_sftp::client::SftpSession::new(stream)` (API vérifiée :
  `new<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>`). Pas de seconde auth.
- Panneau latéral (toggle) : arborescence + liste, fichiers cachés, tri, recherche,
  renommer, supprimer, mkdir, chmod/chown (`set_metadata`), propriétés, copier le chemin,
  symlinks (`symlink`, `read_link`).
- « Suivre le répertoire du terminal » : **OSC 7** (`file://host/chemin`) émis par le
  shell distant ; l'app peut taper dans le shell de la session une ligne fixe (PROMPT_COMMAND/precmd)
  sur action explicite (ou réglage opt-in) — **jamais écrite dans les fichiers distants** ; snippet
  à coller disponible pour un suivi permanent.
- **Moteur de transferts** :
  - file d'attente persistante en mémoire, workers parallèles (défaut 3, max 8) ;
  - progression, débit, ETA via `Channel<TransferProgress>` ;
  - pause/reprise (token d'annulation + offset conservé) ; **reprise de transfert
    interrompu** : fichier local `.part` + offset, vérification taille distante ≥ offset,
    repositionnement (`AsyncSeek` sur `File` russh-sftp — confirmé au M5) ;
  - dossiers récursifs (parcours `read_dir`, politique symlinks : ignorer par défaut) ;
  - préservation permissions + dates (`set_metadata` après complétion) ;
  - intégrité : taille systématiquement ; hash SHA-256 si l'extension SFTP `check-file`
    est offerte par le serveur (rare) ; sinon option « revérification complète »
    (relecture distante, coûteuse, opt-in). **Honnêteté** : SFTP v3 n'a pas de hash
    standard — c'est documenté dans l'UI.
  - upload atomique pour l'édition distante : écriture `.harakats-<uuid>.tmp` distant +
    `rename` (voir §11).

## 11. Édition à distance

> **Statut M6 : implémenté** (`crates/haraka-ssh/src/edit.rs`, `src-tauri/src/edit.rs`, `ui/.../EditorView.svelte`).
> Écarts / précisions par rapport au plan ci-dessous :
> - Le contenu est manipulé en **octets** côté backend (fins de ligne CRLF/LF et octets non UTF-8
>   préservés ; un fichier non UTF-8 n'est éditable qu'en externe). Limite d'édition : 20 Mio.
> - **Remplacement distant** : SFTP v3 n'écrase pas par `rename` et `russh-sftp` n'expose pas
>   `posix-rename@openssh.com` → temporaire distant `.<nom>.haraka-<id>.tmp` (même dossier, droits copiés),
>   puis cible renommée en `.bak`, temporaire renommé, `.bak` supprimé ; en cas d'échec la cible
>   est RESTAURÉE. Jamais de fichier partiel sous son nom ; courte fenêtre sans fichier pendant le
>   remplacement (un lecteur concurrent voit l'ancien contenu, le nouveau, ou une absence brève).
> - Conflit : `mtime/taille` puis SHA-256 (un `touch` n'est pas un conflit) ; fichier supprimé = conflit
>   « n'existe plus » (écraser le recrée).
> - Éditeur externe : commande en **argv** découpée façon shell mais SANS shell (`{file}` ou chemin
>   ajouté), vide = `xdg-open` ; surveillance `notify` du dossier temporaire + débounce 400 ms ;
>   l'écho de notre propre écriture est ignoré (comparaison de hash).
> - Nettoyage : fermeture du document, verrouillage du coffre (`close_all`), purge au démarrage.

1. Ouverture : download vers `data/tmp/<uuid>/<filename>` (répertoire **0700**, fichiers
   0600), enregistrement `mtime + taille + hash SHA-256` du fichier distant à l'instant T.
2. Édition : CodeMirror 6 intégré (coloration par extension, recherche, fins de ligne
   LF/CRLF, tabulations) **ou** éditeur externe configurable (commande lancée par le
   backend, jamais via un plugin shell Tauri).
3. Sauvegarde : watcher `notify` sur le fichier local → à chaque écriture stable
   (debounce 300 ms) : re-stat distant ; si `mtime/taille` changés → download du distant
   dans un second temp, comparaison SHA-256 :
   - identique → upload silencieux ;
   - différent → **dialogue conflit** : écraser / recharger / **comparer** (vue diff
     `@codemirror/merge`).
4. Upload atomique : temp distant même répertoire → `set_metadata` → `rename`.
5. Fermeture : nettoyage du répertoire temp (et à la sortie de l'app, y compris crash :
   purge au démarrage).

## 12. Imports

> **Statut M7 : implémenté** — `crates/haraka-core/src/import/{mod,ssh_config,remmina,mobaxterm}.rs`
> (parseurs purs + planificateur + applicateur sur le `Store`), `src-tauri/src/import_external.rs`
> (dialogue de fichiers, lecture des clés, **aperçu conservé côté backend** : 8 aperçus max, seuls des
> compteurs/noms/avertissements structurés `{code, params}` vont à l'UI), applicateur : groupe
> « Import <source> », noms en conflit suffixés, clés chiffrées par `vault.set_secret`. Bornes : entrée
> 4 Mio, lignes 16 Ko, 5000 connexions, 200 avertissements, Include profondeur 8 / 200 fichiers.

| Source | Méthode | Secrets |
|---|---|---|
| `~/.ssh/config` | parseur maison testé (Host patterns + négation, `Include` glob, User, HostName, Port, IdentityFile, IdentitiesOnly, ProxyJump → chaîne de hops ; `Match` et `ProxyCommand` reportés, documenté). Les hôtes joker (`*`, `?`) deviennent des **défauts de dossier**, pas des connexions. | IdentityFile → clé lue et **importée dans le coffre** (chiffrée) ; passphrase inconnue → `needs_reentry` |
| `.remmina` | parseur INI maison (`protocol=SSH`, server, username, ssh_tunnel_* → tunnels) ; champs inconnus ignorés sans erreur | `password=` chiffré par Remmina avec sa clé locale → **`needs_reentry` par défaut**. Option expérimentale M7 : déchiffrement best-effort si l'utilisateur fournit son `remmina.pref` (sa propre machine, son choix) |
| `.mxtsessions` (MobaXterm) | parseur INI-like maison (sections Bookmarks, SubRep → dossiers, Login/Host/Port) | mots de passe chiffrés (schéma propriétaire MobaXterm) → **`needs_reentry`** ; déchiffrement non garanti, reporté |

Règle commune : **un import ne plante jamais** sur un champ inconnu/corrompu ; il produit
un rapport `{ importés, ignorés, secrets à ressaisir }` et une entrée d'audit. Les
connexions importées atterrissent dans un dossier dédié « Import <source> » pour revue.

## 13. IPC Tauri v2

Principe : **commandes minimales, typées, validées côté backend** (zod-like manuel ou
serde + garde-fous), DTOs sanitisés (aucun champ secret sauf reveal explicite). Les
secrets circulent **frontend → backend** uniquement quand l'utilisateur les saisit
(création/édition de credential) ; **backend → frontend** uniquement via
`credential_reveal` / `clipboard_copy_secret` (action explicite, journalisée).

### Commandes (groupe → exemples)

| Groupe | Commandes |
|---|---|
| Coffre | `vault_status`, `vault_create`, `vault_unlock` (master \| keyring), `vault_lock`, `vault_change_master`, `vault_keyring_set_enabled` |
| Arbre | `tree_get` (DTO sanitisé), `folder_upsert/delete/move`, `connection_upsert/delete/move/duplicate`, `connection_effective_config` (aperçu héritage, sans secrets) |
| Credentials | `credential_list` (métadonnées + `needs_reentry`), `credential_upsert` (secrets saisis par l'utilisateur), `credential_delete`, `credential_reveal(id, field)`, `clipboard_copy_secret(id, field, ttl)` |
| Sessions | `session_open(connection_id \| quick_spec, output_channel: Channel<TermEvent>)`, `session_write`, `session_resize`, `session_close`, `session_reconnect`, `multi_exec_set(sessions, enabled)`, `auth_prompt_respond(prompt_id, answers)` |
| SFTP | `sftp_list/stat/mkdir/rename/remove/chmod/chown/readlink/copy_path`, `sftp_follow_cwd_set` |
| Transferts | `transfer_enqueue(specs, progress_channel)`, `transfer_pause/resume/cancel`, `transfer_list` |
| Édition | `edit_open(connection_id, remote_path)` → contenu + temp path, `edit_conflict_resolve(choice)`, `edit_close` |
| Tunnels | `tunnel_upsert/delete/start/stop`, `tunnel_list` (état live) |
| Export/Import | `export_bundle(scope, passphrase, options)` (dialogue fichier natif via rfd, backend), `import_bundle_preview(path, passphrase)`, `import_bundle_apply(preview_id, strategy)`, `import_ssh_config`, `import_remmina`, `import_mobaxterm` |
| Host keys | `known_hosts_list`, `known_hosts_import_system`, `hostkey_prompt_respond(prompt_id, decision)` |
| Divers | `settings_get/set` (sous-ensemble non secret), `macro_upsert/delete/run`, `audit_log_list`, `palette_search` |

### Événements (backend → UI, sanitisés)

`vault://state` (locked/unlocked/keyring_available), `session://state`,
`auth://prompt` (questions keyboard-interactive, bannière), `hostkey://prompt`
(empreintes TOFU/changement), `transfer://progress` (aussi via Channel),
`tunnel://state`, `import://report`, `lock://reason`.

### Capabilities et CSP

- Capabilities : `core:event:default`, permissions des commandes ci-dessus (déclarées
  explicitement), `core:webview:allow-create-webview-window` + `core:window:default`
  (onglets détachés). **Aucun plugin fs/shell/http/dialog** : fichiers via rfd côté
  backend, processus externe (éditeur) via `std::process` côté backend uniquement.
- Fenêtre principale : `devtools` activé **seulement en build dev**.
- CSP stricte (tauri.conf.json) :

```
default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self' data:; font-src 'self'; connect-src 'self' ipc: http://ipc.localhost;
object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'
```

`'unsafe-inline'` pour les styles uniquement : CodeMirror injecte des styles
dynamiques (pas de nonce possible) ; aucun script inline, aucune URL distante chargée.

## 14. Répertoires de données et mode portable

- Défaut : `$XDG_DATA_HOME/harakats` (→ `~/.local/share/harakats`), **0700** ;
  fichiers **0600** : `vault.hrk`, `backups/vault-<n>.hrk`, `known_hosts`, `audit.log`,
  `logs/` (sessions opt-in), `tmp/` (édition distante, purgé au démarrage).
- **Mode portable** (ajout proposé) : si un fichier marqueur `portable.marker` existe à
  côté du binaire (ou `--data-dir <chemin>`), les données vivent dans `./data/` — clé USB
  ou dépôt synchronisé. Même format, même crypto : c'est l'essence du projet.
- Aucun secret hors du coffre (pas de cache plaintext, pas de « remember password »
  ailleurs que le keyring optionnel décrit §5).

## 15. Tests et CI (détail par jalon dans ROADMAP.md)

- **Serveur SSH de test in-process** : russh est client **et serveur** → un serveur russh
  minimal dans `tests/` (auth password/clé/keyboard-interactive contrôlables, host key
  fixe, canaux shell/sftp/direct-tcpip) pour des tests hermétiques, rapides, sans réseau.
- **Interop réel** : `openssh-server` démarré dans le conteneur de dev (Podman) et sur le
  runner CI (port 2222, config dédiée, clés de test jetables) pour SFTP, tunnels,
  reprises de transfert. Pas de docker-in-docker nécessaire.
- `haraka-core` est headless : le **test A→B** (export profil A, import profil B vierge,
  autre mot de passe maître, ouverture des connexions avec identifiants) tourne en CI
  sans GUI. Falsification : bit modifié en en-tête/payload/secret → rejet (tag AEAD).
- CI GitHub Actions : `cargo fmt --check`, `cargo clippy -D warnings`, `cargo audit`,
  `cargo test` (workspace), `pnpm lint`, `pnpm check` (tsc strict), `pnpm test` (vitest),
  build `.deb` + `.rpm`. (Le dépôt git et le push restent à ta main.)

## 16. Packaging

- `tauri.conf.json` : bundle targets **`deb` + `rpm`** (décision utilisateur du
  2026-11-20 ; l'AppImage est retirée du périmètre v1), icônes, métadonnées
  `com.harakats.desktop`, catégories Network/System.
- **.deb** (Debian 12+ / Ubuntu 22.04+) : dépendances runtime déclarées
  (`libwebkit2gtk-4.1-0`, `libgtk-3-0`, `libsoup-3.0-0`, `libjavascriptcoregtk-4.1-0`,
  `libglib2.0-0`) — `apt install ./harakats_*.deb` les tire automatiquement.
- **.rpm** : produit nativement par le bundler Tauri (`bundle.linux.rpm.depends`, pas
  d'étape rpmbuild manuelle). Cibles vérifiées le 2026-11-20 : **Fedora ≥ 42**
  (webkit2gtk4.1 officiel, 2.48→2.51), **RHEL/Rocky/Alma 10 via EPEL**
  (webkit2gtk4.1-2.48.3 publié), Mageia 9/10. Dépendances RPM déclarées :
  `webkit2gtk4.1`, `javascriptcoregtk4.1`, `gtk3`, `libsoup3`, `glib2` — à confirmer
  par `rpm -qp --requires` sur le paquet produit au M7.
  **EL9 (RHEL/Rocky/Alma 9) NON supporté** : webkit2gtk4.1 absent des dépôts et
  glib2 2.68 < 2.70 requis par la pile soup3 (cf. tauri-apps/tauri#9039).
- Build des **deux formats dans le même conteneur Debian bookworm** (glibc 2.36,
  inférieure à celle de toutes les cibles supportées → compatibilité ascendante).
- Signature GPG des .rpm optionnelle (`TAURI_SIGNING_RPM_KEY` /
  `TAURI_SIGNING_RPM_KEY_PASSPHRASE`, clés jamais dans le dépôt) — décision au M7.
- keyring `vendored` (libdbus liée statiquement) : pas de libdbus requise chez
  l'utilisateur, quel que soit le format.
- Flatpak (le runtime embarque le webview → « tout-en-un ») : option **post-v1**,
  sans impact sur l'architecture.
- Windows/macOS : code portable (pas de `#[cfg(target_os)]` bloquant hors keyring/zbus
  qui ont des backends natifs), **non livrés en v1**.

## 17. Améliorations ajoutées au prompt (proposées)

1. **Chiffrement du store entier** (pas seulement des secrets) : hôtes, utilisateurs et
   topologie sont des métadonnées sensibles (double couche, SECURITY.md §2).
2. **Backups rotatifs du coffre** + restauration guidée en cas de corruption.
3. **Mode portable** (données à côté du binaire) — aligné sur la mission de portabilité.
4. **Journal d'audit local** (déverrouillages, reveals, copies, exports, changements de
   host key) sans aucun secret.
5. **Calibration automatique des paramètres Argon2id** à la création (cible 0,5–1 s).
6. **Serveur SSH de test in-process** (russh server) en plus du conteneur openssh.
7. **i18n FR/EN** dès M2 (FR par défaut).
8. **Keyring non bloquant** : absence de KWallet/Secret Service → repli silencieux
   (exigence utilisateur explicite).
9. Indicateur latence (`send_ping`) et état des tunnels en barre d'état.

## 18. Limites v1 assumées / reportées

- RDP, VNC, Telnet, série : abstraction prête, backends reportés.
- `ProxyCommand` et `Match` dans l'import ssh_config (spawn shell / complexité).
- Agent forwarding SSH (risque sécurité, décision documentée).
- Drag-out de fichiers vers l'OS (limitation webview Tauri v2) ; UDP ASSOCIATE / BIND
  SOCKS5 ; hash distant standard SFTP (extension `check-file` seulement).
- Déchiffrement des mots de passe MobaXterm / Remmina à l'import : `needs_reentry` par
  défaut (best-effort Remmina expérimental en M7).
- Interpolation de secrets dans les macros ; enregistrement de session type asciinema.
- Certificats SSH host/user (`PublicKeyOrCertificate` est prêt côté russh) : post-v1.
- EL9 (RHEL/Rocky/Alma 9) : non supporté en v1 (webkit2gtk4.1 absent, glib2 trop
  ancien pour la pile soup3) ; AppImage retirée du périmètre (décision utilisateur
  du 2026-11-20, au profit de .deb + .rpm).
- Windows/macOS non livrés ; pas de télémétrie, pas de mise à jour automatique (aucune
  URL distante dans la webview).

## 19. Sources (vérification des versions/APIs, 2026-11-20)

- https://crates.io/api/v1/crates/russh (0.65.2, 2026-11-16)
- https://docs.rs/russh/latest/russh/client/struct.Handle.html
- https://docs.rs/russh/latest/russh/client/trait.Handler.html
- https://docs.rs/russh/latest/russh/struct.Channel.html
- https://docs.rs/russh/latest/russh/ (modules keys/agent, pas de ssh_config/known_hosts racine)
- https://crates.io/api/v1/crates/russh-sftp (3.0.1, 2026-09-28)
- https://docs.rs/russh-sftp/latest/russh_sftp/client/struct.SftpSession.html
- https://crates.io/api/v1/crates/argon2 (0.6.0) ; https://docs.rs/argon2/latest/argon2/
- https://crates.io/api/v1/crates/chacha20poly1305 (0.11.0) ; https://docs.rs/aead/latest/aead/ (rand_core 0.10, AeadInOut)
- https://crates.io/api/v1/crates/rand (0.10.4) ; https://crates.io/api/v1/crates/secrecy (0.10.3) ; https://crates.io/api/v1/crates/zeroize (1.9.2)
- https://docs.rs/keyring/3.6.3/keyring/ (features sync-secret-service/crypto-rust/vendored)
- https://v2.tauri.app/release/ (2.12.x, MSRV 1.90) ; https://v2.tauri.app/start/prerequisites/
- https://registry.npmjs.org/svelte/latest (5.58.0) ; https://registry.npmjs.org/@codemirror/view/latest (6.43.15)
- https://v2.tauri.app/distribute/rpm/ (bundling RPM natif, `bundle.linux.rpm.depends`, baseline de build)
- https://pkgs.org/download/webkit2gtk4.1 (Fedora 42–44, EPEL 10, Mageia 9/10 ; absent EL9)
- https://github.com/tauri-apps/tauri/issues/9039 (EL9/CentOS 9 : glib2 2.68 < gio 2.70 requis par soup3)
- https://docs.rs/rand/0.10.4/rand/rngs/struct.SysRng.html (OsRng supprimé en rand 0.10 → SysRng/TryRng)
- https://docs.rs/aead/0.6.1/aead/trait.Aead.html (encrypt/decrypt avec Payload{msg,aad})
- https://docs.rs/zbus/latest/zbus/struct.MessageStream.html (zbus 6.4 : for_match_rule)
- https://crates.io/api/v1/crates/rfd (0.18.0, features xdg-portal/wayland par défaut, gtk3 optionnelle)
- https://v2.tauri.app/security/capabilities/ (commandes applicatives autorisées par défaut)
- https://registry.npmjs.org/vite/latest (8.3.8) ; …/@sveltejs/vite-plugin-svelte/latest (7.3.1) ; …/svelte-check/latest (4.7.8) ; …/vitest/latest (5.1.1) ; …/@tauri-apps/api/latest (2.12.5) ; …/@tauri-apps/cli/latest (2.12.6)

## 11. Internationalisation

- **Catalogues** : `ui/src/locales/<code>.json`, clés plates (`section.nom`), valeurs avec `{param}`
  et pluriels `<clé>_one` / `<clé>_other`. `fr` est la référence (toutes les clés), puis `en` en repli.
  Langues livrées : fr, en, it, de, es, pt. `_name` = nom de la langue dans sa langue.
- **Web** (`ui/src/lib/i18n.svelte.ts`) : `import.meta.glob` découvre les catalogues ; `t`, `tn`
  (Intl.PluralRules), `formatDateTime` (Intl), `setLocale` (mémorise dans localStorage pour l'écran de
  déverrouillage), langue initiale = mémorisée → langue du système → fr. Réactif (`$state`).
- **Natif** (`src-tauri/src/i18n.rs`) : les mêmes JSON embarqués par `include_str!` (clés `native.*`),
  `tr` / `tr_with`. La langue est lue dans les réglages à l'ouverture d'une fenêtre VTE et portée par
  `VteWindow.locale` (menus, dialogues, barre d'état, textes écrits dans le terminal).
- **Erreurs backend** : le Rust renvoie des chaînes françaises ; `ui/src/lib/errors.ts` les mappe
  (regex + paramètres, wrappers récursifs) vers des clés `be.*` — branché sur `message()` (api.ts).
- **Garde-fous** (vitest + cargo test) : parité des clés et des `{param}` entre langues ; aucun texte
  visible en dur dans les `.svelte` (parse AST) ; chaque fichier de `locales/` déclaré dans `LOCALES`.
