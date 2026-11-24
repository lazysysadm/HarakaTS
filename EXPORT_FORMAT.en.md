# EXPORT_FORMAT — HarakaTS v1 (vault and export format)

> **Status: v1 format FROZEN (2026-11-21, v1 delivery).** Any incompatible evolution goes through a
> new major format version (`format_version` incremented) with v1 reading kept;
> the fixtures `tests/fixtures/vault_v1.hrk` and `export_v1.hrkx` and the normative tests (§9) are the
> reference and must not be regenerated.

> Phase 0 — specification of the versioned binary format. To be validated before any code is written.
> This document is the normative reference: the implementation (`haraka-core::vault` and
> `haraka-core::export`) and the round-trip/tampering tests conform to it.

## 1. Overview

Two files share the **same binary envelope**:

| File | Magic (8 ASCII bytes) | Extension | Usage |
|---|---|---|---|
| Local vault | `HRKTSVL1` | `.hrk` | complete application store, encrypted by the local DEK |
| Export bundle | `HRKTSXP1` | `.hrkx` | portable configuration + secrets, encrypted by a key derived from the **export passphrase** |

The magic includes the format name and generation; `format_version` (u16) carries compatible
evolutions. A reader that does not recognize the magic/version **rejects without attempting to
decrypt**.

## 2. Binary envelope

> **Amendment of 2026-11-20 (before any code, M1)**: the initial version of this document placed
> `kdf` and `wrapped_dek` in the header covered by the payload AAD, which made the requirement
> "master password change = unchanged payload" impossible (the salt changes → the AAD changes →
> the payload tag would break). The envelope is therefore split: **immutable header** (identity,
> algorithm) and mutable **KDF + wrapped-DEK block**. Every byte remains authenticated: the header
> by the payload tag, the KDF/wrapped_dek block by the DEK wrap tag (its AAD includes the KDF
> bytes). No security consequence; `format_version` stays 1 (no file was produced before this
> amendment).

All integers are **big-endian**. The file is exactly:

```
┌──────────────────────────────┬────────────┬──────────────────────────────────────────┐
│ offset                       │ size       │ field                                    │
├──────────────────────────────┼────────────┼──────────────────────────────────────────┤
│ 0                            │ 8          │ ASCII magic ("HRKTSVL1" / "HRKTSXP1")    │
│ 8                            │ 2          │ format_version = 1 (u16be)               │
│ 10                           │ 4          │ header_len (u32be, ≤ 1 MiB)              │
│ 14                           │ header_len │ header: canonical UTF-8 JSON (§3.1)      │
│ 14 + header_len              │ 4          │ kdf_len (u32be, ≤ 64 KiB)                │
│ 18 + header_len              │ kdf_len    │ kdf: canonical UTF-8 JSON (§3.3)         │
│ 18 + header_len + kdf_len    │ 24         │ dek_nonce (random, SysRng)               │
│ … + 24                       │ 48         │ wrapped_dek: encrypted 32-byte DEK + 16 tag │
│ … + 48                       │ 24         │ payload_nonce (random, SysRng)           │
│ … + 24                       │ rest       │ payload_ct: ciphertext + 16-byte tag     │
└──────────────────────────────┴────────────┴──────────────────────────────────────────┘
```

- **payload_ct** = XChaCha20-Poly1305(key=DEK, nonce=payload_nonce,
  AAD=`magic ∥ format_version(u16be) ∥ header_len(u32be) ∥ header_bytes`,
  plaintext=UTF-8 JSON payload (§5)). The Poly1305 tag (16 bytes) is appended to the ciphertext
  (`chacha20poly1305` convention).
- **wrapped_dek** = XChaCha20-Poly1305(key=KEK, nonce=dek_nonce, plaintext=32-byte DEK,
  AAD=`"HRKTS/DEK\0" ∥ vault_id_utf8 ∥ kdf_len(u32be) ∥ kdf_bytes`).
- Any modification of a bit of the header, the nonce or the payload → payload tag failure →
  `Tampered` error. Any modification of the KDF block (salt, m, t, p) or of the wrapped DEK →
  wrap tag failure → `WrongPassword`/`Tampered`, **no partial decryption**.
- `header_len` bounded (1 MiB) and `kdf_len` bounded (64 KiB) to forbid abusive allocations on
  import; maximum accepted file size: 256 MiB (guardrail).
- **Master password change**: only `kdf` (new salt) and `wrapped_dek` are rewritten; `header`,
  `payload_nonce` and `payload_ct` are preserved byte for byte → O(1) operation, per the
  requirement.

## 3. Header (canonical JSON)

Canonical = lexically sorted keys, no superfluous whitespace, UTF-8, integer numbers without
exponent. **The exact bytes stored in the file form the AAD**: the header is never re-serialized
for verification, the read bytes are used.

### 3.1 Immutable vault header (`HRKTSVL1`)

```json
{
  "app": "harakats",
  "app_version": "0.1.0",
  "created_at": "2026-11-20T12:00:00Z",
  "payload_alg": "XChaCha20-Poly1305",
  "vault_id": "<uuid v4>"
}
```

Covered by the payload AAD (§2). `updated_at` lives in the payload (store), not in the header, so
the header stays immutable.

### 3.2 Immutable export header (`HRKTSXP1`)

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
  "export_id": "<uuid v4, specific to the bundle>",
  "exported_at": "2026-11-20T12:00:00Z",
  "payload_alg": "XChaCha20-Poly1305",
  "source_vault_id": "<uuid of the origin vault>"
}
```

- `scope`: `"all"` | `"folder"` (with `folder_id`, complete subtree) | `"no-secrets"`
  (`includes_secrets=false`, EncSecret fields absent from the payload).
- `export_id` serves as the AAD root for the bundle's secrets (§4): an exported secret cannot be
  re-injected as-is into another vault/another entry.

### 3.3 KDF + wrapped-DEK block (mutable, common to both magics)

```json
{
  "alg": "argon2id",
  "m_cost": 131072,
  "p_cost": 4,
  "salt": "<base64 16 bytes>",
  "t_cost": 3,
  "version": "0x13"
}
```

Followed in the file by `dek_nonce` (24 bytes) and `wrapped_dek` (48 bytes), see §2.

- **KEK** = Argon2id(master password for the vault, **export passphrase** for the bundle; salt,
  m_cost KiB, t_cost, p_cost) → 32 bytes (`argon2::Argon2::hash_password_into`, raw API).
- **wrapped_dek** = XChaCha20-Poly1305(KEK, dek_nonce, DEK,
  AAD=`"HRKTS/DEK\0" ∥ container_id_utf8 ∥ kdf_len(u32be) ∥ kdf_bytes`).
- The KDF block is in clear text (normal: these are parameters, not secrets) and **authenticated
  by the wrap tag**: fraudulently lowering m/t/p or changing the salt makes the unwrap fail →
  rejection.
- Changing the master password: new `salt`, new `wrapped_dek`; **header and payload unchanged**
  (§2).

## 4. Per-secret encryption (layer 2) and canonical AAD

Each secret field of the payload (`credentials[].password`, `credentials[].private_key`,
`credentials[].passphrase`, `macros[].content`) is an object:

```json
{ "alg": "XChaCha20-Poly1305", "nonce": "<b64 24 bytes>", "ct": "<b64 ciphertext+tag>" }
```

- Key: the container's DEK (local for `.hrk`, export for `.hrkx`).
- **Canonical AAD** (anti-permutation, prompt requirement):

```
AAD_secret = u32be(len(id_conteneur)) ∥ id_conteneur_utf8
           ∥ u32be(len(entry_id))     ∥ entry_id_utf8
           ∥ u32be(len(field_name))   ∥ field_name_utf8
```

`id_conteneur` = `vault_id` (vault) or `export_id` (bundle). `field_name` ∈
`{"password","private_key","passphrase","macro_content"}`. The length prefixes forbid any
concatenation ambiguity.

- 24-byte nonce **random and unique per (re)encryption** — never a counter reused across files,
  never a content-derived nonce.
- A secret whose AAD does not match (moved across entries/fields/vaults) → tag failure → the
  whole import is rejected (`Tampered`), no silent repair.

## 5. Payload (plaintext JSON, before encryption)

### 5.1 Vault

The complete store described in ARCHITECTURE.en.md §4 (`schema_version`, `vault_id`, `settings`,
`folders`, `connections`, `credentials` with EncSecret, `macros`, `recents`, `ui_state`). The
payload's `vault_id` == the header's `vault_id` (checked at decryption).

### 5.2 Export

```json
{
  "schema_version": 1,
  "export_id": "…",
  "source_vault_id": "…",
  "folders": [ …exported subtree, parent_id null for the exported root… ],
  "connections": [ … ],
  "credentials": [ …secrets re-encrypted with the export DEK, AAD=export_id… ],
  "macros": [ … ],
  "settings": null
}
```

- Local settings (`settings`) are **not exported** (they describe the machine, not the
  connections). `recents`/`ui_state` not exported.
- References (`credential_id`, `folder_id`, `jump_chain[].credential_id`) are included and
  resolved within the bundle: a folder export embeds the credentials referenced by its subtree
  (deduplicated).
- `no-secrets` scope: Credential objects keep `username`, `name`, `needs_reentry` = all secret
  fields; no EncSecret in the bundle.

## 6. Export procedure

1. Vault unlocked (DEK in memory). The user chooses: scope (everything / folder / no secrets),
   target file (native backend dialog `rfd`), **export passphrase** (typed twice, strength
   indicator, ≥ 8 characters).
2. Generation of `export_id` (uuid v4) + export DEK (SysRng) + 16-byte salt (SysRng).
3. Export KDF calibration: same 0.5–1 s target parameters ("fast" setting possible for CI tests
   only, never by default in the UI).
4. KEK_export = Argon2id(passphrase, salt, m, t, p); wrap DEK_export (AAD §3.1).
5. Payload construction: subtree selection, **re-encryption of every secret** with DEK_export and
   AAD = export_id ∥ entry_id ∥ field (secrets are never written in clear text, even temporarily:
   decryption → re-encryption in memory, zeroized buffers).
6. Canonical serialization, layer-1 encryption (payload AAD §2), atomic write of the `.hrkx`
   (0600), fsync.
7. Audit entry: `export {scope, includes_secrets, counts}` (no passphrase).

## 7. Import procedure

1. File reading, magic + `format_version` check (clean rejection if unknown; `format_version` >
   supported → message "newer version, update HarakaTS").
2. KEK derivation from the typed passphrase (header parameters), export DEK unwrap →
   `WrongPassword` on failure (generic message, no crypto detail).
3. Layer-1 decryption (payload AAD §2, read header bytes) → `Tampered` on failure.
4. Structural validation: supported `schema_version`, valid UUIDs, consistent internal
   references, `export_id` == header. Then decryption **of every secret** (AAD export_id ∥
   entry_id ∥ field) → any failure = complete rejection.
5. **Preview**: bundle tree, counters, list of UUID conflicts with the local vault. The user
   chooses the strategy **per entry or globally**:
   - `keep`: the local entry wins, the imported entry is ignored;
   - `replace`: the imported entry replaces the local one (same UUID);
   - `duplicate`: new UUID for the imported entry, **remapping of all references**
     (credential_id, folder_id, jump_chain) within the duplicated subtree.
6. **Re-encryption with the local vault's DEK**: each imported secret is decrypted (export key)
   then re-encrypted (local DEK, AAD = vault_id_local ∥ entry_id ∥ field). Secrets never leave
   backend memory.
7. Merge into the store, `updated_at`, atomic write + backup, audit
   `import {source, strategy, counts}`.
8. **No partial write**: the merge is transactional (complete store in memory → a single rename).
   A failed import leaves the vault intact.

## 8. Compatibility and evolution

- `format_version=1`: this document. Optional field additions in the header/payload = tolerated
  minor version (a v1 reader ignores unknown JSON fields, but the AAD covers the exact bytes: a
  file produced by a future version remains authenticated; unknown semantics → clean rejection if
  `schema_version` is unsupported).
- Algorithm/KDF/AAD change → `format_version=2` with a documented migration path; magics stay
  stable per generation.
- A `.hrk` file **never** opens as `.hrkx` and vice versa (distinct magic); renaming the extension
  changes nothing (the magic is authoritative).
- No compression in v1 (the JSON payload is small; compression before AEAD would open CRIME-like
  oracles — documented decision).

## 9. Normative tests (CI, milestone M1 and at every milestone)

| Test | Expected |
|---|---|
| Vault round trip: create → write → lock → unlock(master) | store identical byte for byte (except timestamps) |
| Export round trip: A → bundle → virgin B (other data dir, **other master password**) | all of B's connections open with the credentials (criterion #1; headless `haraka-core` test + SSH opening test against a test server) |
| Tampering: 1 bit flip in magic / version / header_len / header / kdf_len / kdf / dek_nonce / wrapped_dek / payload_nonce / payload / tag | rejection (`Tampered`, `WrongPassword` or `BadFormat`), no partial payload |
| Permutation: secret of entry X copied into entry Y (same vault) | AAD failure → rejection |
| Cross-field permutation: `password` ↔ `passphrase` | AAD failure → rejection |
| Wrong passphrase (export and vault) | `WrongPassword`, generic message |
| Master change | identical payload, re-wrapped header, old unlock refused / new accepted |
| `no-secrets` export | no EncSecret in the bundle; import → `needs_reentry` |
| Folder export | subtree + referenced credentials only |
| Conflict import: keep / replace / duplicate | consistent UUIDs and references, no orphan secret |
| Truncated file / trailing bytes import | clean rejection |
| Unknown `format_version` | clean rejection, explicit message |
| Nonces: two successive exports of the same content | all nonces distinct (SysRng) |
| Permissions | files created 0600, directories 0700 |

Test vectors: a fixture set (`tests/fixtures/vault_v1.hrk`, `export_v1.hrkx`, public test
passphrases) is generated at M1 and versioned to detect any format regression.