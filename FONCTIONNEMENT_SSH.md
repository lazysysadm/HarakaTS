# FONCTIONNEMENT — La connexion SSH dans HarakaTS

🇬🇧 [English version](FONCTIONNEMENT_SSH.en.md) · 🇫🇷 Français

> Document explicatif (lecture seule, aucun changement de code). Décrit le chemin complet
> d'une connexion SSH, du clic sur ▶ à l'affichage dans le terminal natif, tel qu'implémenté
> dans `crates/haraka-ssh/src/session.rs` et `src-tauri/src/sessions.rs` (v1.0.8).
> Complète : [ARCHITECTURE.md](ARCHITECTURE.md) §7, [SECURITY.md](SECURITY.md) §7.

## 1. Vue d'ensemble

```
clic ▶ / double-clic / Ctrl+K
   │
   ▼
[1] Résolution backend (config effective + identifiants)
   │
   ▼
[2] Ouverture fenêtre VTE native → dimensions réelles (cols×rows)
   │
   ▼
[3] TCP → handshake SSH → [4] vérification clé d'hôte → [5] authentification
   │        (chaîne de rebonds si jump hosts)
   ▼
[6] Canal shell : PTY + shell, scindé en lecture/écriture
   │
   ├── sortie : Handler::data → file → décodage/OSC52/journal → VTE (affichage)
   ├── entrée : clavier VTE → tâche ordonnée → écriture SSH
   └── resize : taille fenêtre → window_change → PTY serveur
```

Une connexion = **un canal SSH russh** (avec rebonds imbriqués si besoin), authentifié côté
backend, vérifié anti-MITM, dont le canal shell est pompé en continu vers un terminal **natif
VTE**. Sur la même connexion peuvent se greffer un canal SFTP et des tunnels, **sans seconde
authentification**.

## 2. Les étapes en détail

### [1] Résolution côté backend
`build_open_request` calcule la **config effective** par héritage (système → groupes →
connexion : hôte, port, utilisateur, politique presse-papier, keepalive, encodage…) et les
**matériaux d'authentification** via `materials_for`, dans l'ordre effectif (agent, clé, mot de
passe, keyboard-interactive).

**Les secrets ne quittent jamais le backend** : l'interface ne reçoit que des métadonnées
(sanitized DTOs). Le matériel est résolu depuis le coffre déverrouillé, en mémoire.

### [2] Fenêtre native
Une fenêtre **GTK+VTE** est créée (`src-tauri/src/vte/`) et le PTY serveur n'est ouvert qu'à
ses **dimensions réelles** (cols×rows). Sinon htop/vim/tmux sont dessinés pour une mauvaise
taille (commandes derrière le prompt, top désordonné).

### [3] Transport
- Connexion TCP vers `hôte:port` (`russh::client::connect`, ou `connect_stream` au-dessus d'un
  rebond) puis handshake SSH, négociation des algorithmes (kex, ciphers, host keys).
- **Jump hosts** : connexion au bastion → `channel_open_direct_tcpip(hôte_suivant)` →
  `into_stream()` → nouvelle connexion SSH imbriquée (`connect_stream`), jusqu'à la cible.
  Chaque rebond a son propre credential (héritage inclus). `ProxyJump` importé de ssh_config
  mappe directement cette chaîne.

### [4] Clé d'hôte (anti-MITM)
`Handler::check_server_key` → `verify_host_key` :
- empreinte **SHA256** (format OpenSSH) comparée au `known_hosts` interne ;
- **connue et identique** → accepté ;
- **inconnue** → **TOFU** : dialogue affichant l'empreinte, « Accepter et mémoriser » ou
  « Refuser » ; timeout 300 s → refus ;
- **changée** → **blocage dur** : aucun bouton accepter n'est proposé ; remplacement
  uniquement via Paramètres → Hôtes connus → **Oublier** (audité), puis nouveau TOFU ;
- **fichier illisible** (erreur I/O, octets invalides) → **refus** (fail-closed) ; une ligne
  corrompue pour l'hôte compte comme clé changée. Les entrées `@revoked` sont refusées sans
  prompt.

Aucune décision automatique : l'utilisateur tranche explicitement (sauf refus par défaut).

### [5] Authentification
`authenticate` essaie les matériaux dans l'ordre effectif :
- **agent SSH** (`SSH_AUTH_SOCK`, clés FIDO2 `sk-` incluses ; **pas de forwarding d'agent**) ;
- **clé privée** (déchiffrée avec la passphrase du coffre ; hash RSA négocié via
  `best_supported_rsa_hash`) ;
- **mot de passe** ;
- **keyboard-interactive** (2FA/OTP) : boucle de prompts affichés à l'écran ; le prefill
  automatique est limité au prompt « password » (jamais envoyé à un prompt OTP).

**Repli et erreurs.** Si toutes les méthodes configurées échouent, `authenticate` note les
méthodes que le serveur propose encore. S'il accepte le mot de passe, une invite
« Mot de passe de user@hôte » s'ouvre (puis keyboard-interactive si proposé) au lieu d'un échec
sec. Le message d'erreur final liste les méthodes **essayées** et les méthodes **proposées par le
serveur** (ex. RouterOS refuse les clés ed25519 : il propose `publickey,password`). Un refus
d'authentification n'est jamais rejoué par la reconnexion automatique.

Le matériel est **zeroizé** après usage et au verrouillage du coffre. Les réponses OTP sont
des `Zeroizing<String>`.

### [6] Canal shell
`channel_open_session` → `request_pty(term, cols, rows)` → `request_shell(true)`.

Le canal est **scindé** (`channel.split()`) :
- une **tâche de drain** consomme la moitié lecture (`read_half.wait()`) — obligatoire : la
  file interne de russh est bornée (`channel_buffer_size` = 100) ; sans drain, la boucle de
  session se bloque au ~100ᵉ message (c'était la cause du gel htop) ;
- la moitié écriture (`ChannelWriteHalf`) sert à `data_bytes` / `window_change`.

L'identifiant du **canal shell** est mémorisé (`shell_channel`) pour que `Handler::data`
n'envoie au terminal **que ses données** — les octets SFTP d'un autre canal ne doivent jamais
s'afficher dans le terminal.

## 3. Flux continus

### Sortie (serveur → écran)
`Handler::data` → file mpsc → `spawn_output_forwarder` :
1. décodage `encoding_rs` (UTF-8 par défaut, charset configurable par connexion) ;
2. extraction **OSC 52** (politique presse-papier héritée ; les séquences n'atteignent jamais
   le terminal) ;
3. coloration syntaxique (règles fixes, jamais dans un programme plein écran) ;
4. journal de session (opt-in, chiffré, après extraction OSC 52) ;
5. **lots 16 ms / 32 Ko** → `vte_terminal_feed` (affichage natif, un seul envoi GTK en vol).

### Entrée (clavier → serveur)
Signal `commit` de VTE → `SessionCmd::Input` → **une tâche ordonnée par session** (encode vers
le charset de la connexion) → `channel.data_bytes`. Saisie et resize ne peuvent pas se
croiser ni s'inverser.

### Resize
`size-allocate` de VTE (lu en **idle**, grille à jour) → `SessionCmd::Resize` →
`window_change` → SIGWINCH côté serveur (debounce 100 ms). La taille est mémorisée backend
pour les reconnexions.

### Après la connexion (suivi du répertoire)
Si l'option « suivi du terminal » est active, HarakaTS peut taper dans le shell une ligne
(syntaxe bash/zsh) qui émet l'OSC 7 à chaque prompt. Elle n'est injectée **que** si le serveur
s'annonce **OpenSSH** (`SSH-2.0-OpenSSH…`) et si le prompt ne se termine pas par `>` ; MikroTik
(ROSSSH), Cisco, Dropbear/BusyBox et identification inconnue ne reçoivent jamais cette ligne
(elle pouvait bloquer leur CLI). L'activation manuelle reste possible depuis le panneau SFTP.

### Keepalive
Keepalive russh (intervalle configurable par connexion, 3 échecs max avant déconnexion).

### Reconnexion
Backoff 1→2→4→…→30 s, max 5 tentatives, **uniquement coffre déverrouillé** ; ré-authentification
depuis le cache mémoire backend. Échec final → état « déconnecté », bouton Reconnecter.

### Verrouillage
Toutes les sessions ferment, DEK et matériel zeroizés, fichiers distants déchiffrés effacés,
presse-papier secret effacé (indicateur `x-kde-passwordManagerHint` pour Klipper).

## 4. Points de sécurité clés
- Secrets résolus et conservés **côté backend uniquement** ; reveal/copy explicites et audités.
- **Anti-MITM** : TOFU + blocage dur sur changement + fail-closed sur fichier illisible.
- **Pas de forwarding d'agent** (un serveur distant compromis ne peut pas utiliser l'agent).
- Éditeur externe lancé en **argv sans shell** (pas d'injection).
- Canaux SFTP/tunnels sur la même connexion, sans ré-auth ; bind loopback par défaut.
- Détails : [SECURITY.md](SECURITY.md) §7.

## 5. Fichiers de référence
| Élément | Fichier |
|---|---|
| Manager de sessions, handler, auth, rebonds | `crates/haraka-ssh/src/session.rs` |
| Résolution config + matériaux, forwarder, commandes IPC | `src-tauri/src/sessions.rs` |
| Fenêtre/terminal natif VTE | `src-tauri/src/vte/` |
| Vérification known_hosts | `crates/haraka-core/src/knownhosts.rs` |
| SFTP (2ᵉ canal) | `crates/haraka-ssh/src/sftp.rs` |
| Tunnels L/R/D + SOCKS5 | `crates/haraka-ssh/src/tunnel.rs` |
| OSC 52 / presse-papier | `crates/haraka-core/src/osc52.rs` |
| Journal de session chiffré | `crates/haraka-core/src/sessionlog.rs` |