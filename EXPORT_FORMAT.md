# EXPORT_FORMAT — HarakaTS v1 (format de coffre et d'export)

> **Statut : format v1 FIGÉ (2026-11-21, livraison v1).** Toute évolution incompatible passe par une
> nouvelle version majeure du format (`format_version` incrémentée) avec lecture de la v1 conservée ;
> les fixtures `tests/fixtures/vault_v1.hrk` et `export_v1.hrkx` et les tests normatifs (§9) sont la
> référence et ne doivent pas être régénérés.

> Phase 0 — spécification du format binaire versionné. À valider avant écriture de code.
> Ce document est la référence normative : l'implémentation (`haraka-core::vault` et
> `haraka-core::export`) et les tests de round-trip/falsification s'y conforment.

## 1. Vue d'ensemble

Deux fichiers partagent la **même enveloppe binaire** :

| Fichier | Magic (8 octets ASCII) | Extension | Usage |
|---|---|---|---|
| Coffre local | `HRKTSVL1` | `.hrk` | store complet de l'application, chiffré par la DEK locale |
| Bundle d'export | `HRKTSXP1` | `.hrkx` | configuration + secrets portable, chiffré par une clé dérivée de la **passphrase d'export** |

Le magic inclut le nom et la génération du format ; `format_version` (u16) porte les
évolutions compatibles. Un lecteur qui ne reconnaît pas magic/version **rejette sans
tenter de déchiffrer**.

## 2. Enveloppe binaire

> **Amendement du 2026-11-20 (avant tout code, M1)** : la version initiale de ce
> document mettait `kdf` et `wrapped_dek` dans l'en-tête couvert par l'AAD du payload,
> ce qui rendait impossible l'exigence « changement de mot de passe maître = payload
> inchangé » (le sel change → l'AAD change → le tag du payload casserait). L'enveloppe
> est donc scindée : **en-tête immuable** (identité, algorithme) et **bloc KDF + DEK
> wrappée** mutable. Chaque octet reste authentifié : l'en-tête par le tag du payload,
> le bloc KDF/wrapped_dek par le tag du wrap de la DEK (son AAD inclut les octets KDF).
> Aucune conséquence de sécurité ; `format_version` reste 1 (aucun fichier n'a été
> produit avant cet amendement).

Tous les entiers sont **big-endian**. Le fichier est exactement :

```
┌──────────────────────────────┬────────────┬──────────────────────────────────────────┐
│ offset                       │ taille     │ champ                                    │
├──────────────────────────────┼────────────┼──────────────────────────────────────────┤
│ 0                            │ 8          │ magic ASCII ("HRKTSVL1" / "HRKTSXP1")    │
│ 8                            │ 2          │ format_version = 1 (u16be)               │
│ 10                           │ 4          │ header_len (u32be, ≤ 1 MiB)              │
│ 14                           │ header_len │ header : JSON UTF-8 canonique (§3.1)     │
│ 14 + header_len              │ 4          │ kdf_len (u32be, ≤ 64 KiB)                │
│ 18 + header_len              │ kdf_len    │ kdf : JSON UTF-8 canonique (§3.3)        │
│ 18 + header_len + kdf_len    │ 24         │ dek_nonce (aléatoire, SysRng)            │
│ … + 24                       │ 48         │ wrapped_dek : DEK 32 o chiffrée + tag 16 │
│ … + 48                       │ 24         │ payload_nonce (aléatoire, SysRng)        │
│ … + 24                       │ reste      │ payload_ct : ciphertext + tag 16 o       │
└──────────────────────────────┴────────────┴──────────────────────────────────────────┘
```

- **payload_ct** = XChaCha20-Poly1305(clé=DEK, nonce=payload_nonce,
  AAD=`magic ∥ format_version(u16be) ∥ header_len(u32be) ∥ header_bytes`,
  plaintext=payload JSON UTF-8 (§5)). Le tag Poly1305 (16 o) est suffixé au ciphertext
  (convention `chacha20poly1305`).
- **wrapped_dek** = XChaCha20-Poly1305(clé=KEK, nonce=dek_nonce, plaintext=DEK 32 o,
  AAD=`"HRKTS/DEK\0" ∥ vault_id_utf8 ∥ kdf_len(u32be) ∥ kdf_bytes`).
- Toute modification d'un bit de l'en-tête, du nonce ou du payload → échec du tag du
  payload → erreur `Tampered`. Toute modification du bloc KDF (sel, m, t, p) ou de la
  DEK wrappée → échec du tag du wrap → `WrongPassword`/`Tampered`, **aucun déchiffrement
  partiel**.
- `header_len` borné (1 MiB) et `kdf_len` borné (64 KiB) pour interdire les allocations
  abusives à l'import ; taille maximale de fichier acceptée : 256 MiB (garde-fou).
- **Changement de mot de passe maître** : seuls `kdf` (nouveau sel) et `wrapped_dek`
  sont réécrits ; `header`, `payload_nonce` et `payload_ct` sont conservés octet pour
  octet → opération O(1), conformément à l'exigence.

## 3. En-tête (JSON canonique)

Canonique = clés triées lexicalement, pas d'espaces superflus, UTF-8, nombres entiers
sans exposant. **Les octets exacts stockés dans le fichier font l'AAD** : on ne
re-sérialise jamais l'en-tête pour vérifier, on utilise les octets lus.

### 3.1 En-tête immuable du coffre (`HRKTSVL1`)

```json
{
  "app": "harakats",
  "app_version": "0.1.0",
  "created_at": "2026-11-20T12:00:00Z",
  "payload_alg": "XChaCha20-Poly1305",
  "vault_id": "<uuid v4>"
}
```

Couvert par l'AAD du payload (§2). `updated_at` vit dans le payload (store), pas dans
l'en-tête, pour que l'en-tête reste immuable.

### 3.2 En-tête immuable de l'export (`HRKTSXP1`)

```json
{
  "app": "harakats",
  "app_version": "0.1.0",
  "content": {
    "counts": { "connections": 12, "credentials": 3, "folders": 4, "macros": 2 },
    "folder_id": null,
    "includes_secrets": true,
    "scope": "all"
  },
  "export_id": "<uuid v4, propre au bundle>",
  "exported_at": "2026-11-20T12:00:00Z",
  "payload_alg": "XChaCha20-Poly1305",
  "source_vault_id": "<uuid du coffre d'origine>"
}
```

- `scope` : `"all"` | `"folder"` (avec `folder_id`, sous-arbre complet) | `"no-secrets"`
  (`includes_secrets=false`, les champs EncSecret sont absents du payload).
- `export_id` sert de racine d'AAD aux secrets du bundle (§4) : un secret exporté ne
  peut pas être réinjecté tel quel dans un autre coffre/une autre entrée.

### 3.3 Bloc KDF + DEK wrappée (mutable, commun aux deux magics)

```json
{
  "alg": "argon2id",
  "m_cost": 131072,
  "p_cost": 4,
  "salt": "<base64 16 octets>",
  "t_cost": 3,
  "version": "0x13"
}
```

Suivi dans le fichier de `dek_nonce` (24 o) et `wrapped_dek` (48 o), voir §2.

- **KEK** = Argon2id(mot de passe maître pour le coffre, **passphrase d'export** pour le
  bundle ; sel, m_cost KiB, t_cost, p_cost) → 32 octets
  (`argon2::Argon2::hash_password_into`, API brute).
- **wrapped_dek** = XChaCha20-Poly1305(KEK, dek_nonce, DEK,
  AAD=`"HRKTS/DEK\0" ∥ id_conteneur_utf8 ∥ kdf_len(u32be) ∥ kdf_bytes`).
- Le bloc KDF est en clair (normal : ce sont des paramètres, pas des secrets) et
  **authentifié par le tag du wrap** : abaisser frauduleusement m/t/p ou changer le sel
  fait échouer le unwrap → rejet.
- Changer le mot de passe maître : nouveau `salt`, nouveau `wrapped_dek` ;
  **en-tête et payload inchangés** (§2).

## 4. Chiffrement par secret (couche 2) et AAD canonique

Chaque champ secret du payload (`credentials[].password`, `credentials[].private_key`,
`credentials[].passphrase`, `macros[].content`) est un objet :

```json
{ "alg": "XChaCha20-Poly1305", "nonce": "<b64 24 o>", "ct": "<b64 ciphertext+tag>" }
```

- Clé : DEK du conteneur (locale pour `.hrk`, d'export pour `.hrkx`).
- **AAD canonique** (anti-permutation, exigence du prompt) :

```
AAD_secret = u32be(len(id_conteneur)) ∥ id_conteneur_utf8
           ∥ u32be(len(entry_id))     ∥ entry_id_utf8
           ∥ u32be(len(field_name))   ∥ field_name_utf8
```

`id_conteneur` = `vault_id` (coffre) ou `export_id` (bundle). `field_name` ∈
`{"password","private_key","passphrase","macro_content"}`. Les préfixes de longueur
interdisent toute ambiguïté de concaténation.

- Nonce 24 o **aléatoire unique à chaque (ré)encryption** — jamais de compteur réutilisé
  entre fichiers, jamais de nonce dérivé du contenu.
- Un secret dont l'AAD ne correspond pas (déplacé d'entrée/de champ/de coffre) →
  échec du tag → l'import entier est rejeté (`Tampered`), pas de réparation silencieuse.

## 5. Payload (plaintext JSON, avant chiffrement)

### 5.1 Coffre

Le store complet décrit dans ARCHITECTURE.md §4 (`schema_version`, `vault_id`,
`settings`, `folders`, `connections`, `credentials` avec EncSecret, `macros`, `recents`,
`ui_state`). `vault_id` du payload == `vault_id` de l'en-tête (vérifié au déchiffrement).

### 5.2 Export

```json
{
  "schema_version": 1,
  "export_id": "…",
  "source_vault_id": "…",
  "folders": [ …sous-arbre exporté, parent_id null pour la racine exportée… ],
  "connections": [ … ],
  "credentials": [ …secrets ré-chiffrés avec la DEK d'export, AAD=export_id… ],
  "macros": [ … ],
  "settings": null
}
```

- Les réglages locaux (`settings`) ne sont **pas exportés** (ils décrivent le poste,
  pas les connexions). `recents`/`ui_state` non exportés.
- Les références (`credential_id`, `folder_id`, `jump_chain[].credential_id`) sont
  incluses et résolues dans le bundle : un export de dossier embarque les credentials
  référencés par son sous-arbre (dédupliqués).
- Scope `no-secrets` : les objets Credential conservent `username`, `name`,
  `needs_reentry` = tous les champs secrets ; aucun EncSecret dans le bundle.

## 6. Procédure d'export

1. Coffre déverrouillé (DEK en mémoire). L'utilisateur choisit : portée (tout / dossier /
   sans secrets), fichier cible (dialogue natif backend `rfd`), **passphrase d'export**
   (saisie deux fois, indicateur de force, ≥ 8 caractères).
2. Génération `export_id` (uuid v4) + DEK d'export (SysRng) + sel 16 o (SysRng).
3. Calibration KDF export : mêmes paramètres cibles 0,5–1 s (réglage possible « rapide »
   pour les tests CI uniquement, jamais par défaut dans l'UI).
4. KEK_export = Argon2id(passphrase, sel, m, t, p) ; wrap DEK_export (AAD §3.1).
5. Construction du payload : sélection du sous-arbre, **re-chiffrement de chaque secret**
   avec DEK_export et AAD = export_id ∥ entry_id ∥ field (les secrets ne sont jamais
   écrits en clair, même temporairement : déchiffrement → re-chiffrement en mémoire,
   buffers zeroizés).
6. Sérialisation canonique, chiffrement couche 1 (AAD payload §2), écriture
   atomique du `.hrkx` (0600), fsync.
7. Entrée d'audit : `export {scope, includes_secrets, counts}` (pas de passphrase).

## 7. Procédure d'import

1. Lecture du fichier, vérification magic + `format_version` (rejet propre si inconnu ;
   `format_version` > supporté → message « version plus récente, mettez HarakaTS à jour »).
2. Dérivation KEK depuis la passphrase saisie (paramètres de l'en-tête), unwrap DEK
   d'export → `WrongPassword` si échec (message générique, pas de détail crypto).
3. Déchiffrement couche 1 (AAD payload §2, octets d'en-tête lus) → `Tampered` si échec.
4. Validation structurelle : `schema_version` supporté, UUIDs valides, références
   internes cohérentes, `export_id` == en-tête. Puis déchiffrement **de chaque secret**
   (AAD export_id ∥ entry_id ∥ field) → tout échec = rejet complet.
5. **Aperçu** (preview) : arbre du bundle, compteurs, liste des conflits UUID avec le
   coffre local. L'utilisateur choisit la stratégie **par entrée ou globalement** :
   - `keep` : l'entrée locale prime, l'entrée importée est ignorée ;
   - `replace` : l'entrée importée remplace la locale (même UUID) ;
   - `duplicate` : nouvel UUID pour l'entrée importée, **remappage de toutes les
     références** (credential_id, folder_id, jump_chain) dans le sous-arbre dupliqué.
6. **Re-chiffrement avec la DEK du coffre local** : chaque secret importé est déchiffré
   (clé export) puis re-chiffré (DEK locale, AAD = vault_id_local ∥ entry_id ∥ field).
   Les secrets ne quittent jamais la mémoire backend.
7. Fusion dans le store, `updated_at`, écriture atomique + backup, audit
   `import {source, strategy, counts}`.
8. **Aucune écriture partielle** : la fusion est transactionnelle (store complet en
   mémoire → un seul rename). Un import échoué laisse le coffre intact.

## 8. Compatibilité et évolution

- `format_version=1` : ce document. Ajouts de champs optionnels dans le header/payload
  = version mineure tolérée (lecteur v1 ignore les champs inconnus du JSON, mais l'AAD
  couvre les octets exacts : un fichier produit par une version future reste
  authentifié ; sémantique inconnue → rejet propre si `schema_version` non supporté).
- Changement d'algorithme/KDF/AAD → `format_version=2` avec chemin de migration
  documenté ; les magic restent stables par génération.
- Un fichier `.hrk` ne s'ouvre **jamais** comme `.hrkx` et inversement (magic distinct) ;
  renommer l'extension ne change rien (le magic fait foi).
- Pas de compression en v1 (le payload JSON est petit ; la compression avant AEAD
  ouvrirait des oracles type CRIME — décision documentée).

## 9. Tests normatifs (CI, jalon M1 et à chaque jalon)

| Test | Attendu |
|---|---|
| Round-trip coffre : create → write → lock → unlock(master) | store identique octet à octet (hors timestamps) |
| Round-trip export : A → bundle → B vierge (autre data dir, **autre mot de passe maître**) | toutes les connexions de B s'ouvrent avec les identifiants (critère n°1 ; test headless `haraka-core` + test d'ouverture SSH contre serveur de test) |
| Falsification : 1 bit flip dans magic / version / header_len / header / kdf_len / kdf / dek_nonce / wrapped_dek / payload_nonce / payload / tag | rejet (`Tampered`, `WrongPassword` ou `BadFormat`), aucun payload partiel |
| Permutation : secret de l'entrée X copié dans l'entrée Y (même coffre) | échec AAD → rejet |
| Permutation inter-champs : `password` ↔ `passphrase` | échec AAD → rejet |
| Mauvaise passphrase (export et coffre) | `WrongPassword`, message générique |
| Changement de master | payload identique, header re-wrappé, unlock ancien refusé / nouveau accepté |
| Export `no-secrets` | aucun EncSecret dans le bundle ; import → `needs_reentry` |
| Export dossier | sous-arbre + credentials référencés uniquement |
| Import conflits : keep / replace / duplicate | UUIDs et références cohérents, aucun secret orphelin |
| Import fichier tronqué / trailing bytes | rejet propre |
| `format_version` inconnu | rejet propre, message explicite |
| Nonces : deux exports successifs du même contenu | nonces tous distincts (SysRng) |
| Permissions | fichiers créés en 0600, répertoires 0700 |

Vecteurs de test : un jeu de fixtures (`tests/fixtures/vault_v1.hrk`, `export_v1.hrkx`,
passphrases de test publiques) sera généré au M1 et versionné pour détecter toute
régression de format.
