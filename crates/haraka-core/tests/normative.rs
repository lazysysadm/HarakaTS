//! Tests normatifs EXPORT_FORMAT.md §9 — critère d'acceptation n°1 de HarakaTS.
//! Headless : aucune GUI, aucun réseau, aucun keyring requis.

mod common;

use std::collections::HashMap;
use std::fs;

use common::*;
use haraka_core::crypto::KdfParams;
use haraka_core::export::{self, ExportScope, Strategy};
use haraka_core::format::{Envelope, FORMAT_VERSION, VAULT_MAGIC};
use haraka_core::store::DataDir;
use haraka_core::vault::{SecretField, Vault};
use haraka_core::{Error, effective};
use secrecy::ExposeSecret;
use tempfile::tempdir;

fn dd(root: &std::path::Path, name: &str) -> DataDir {
    DataDir::open(root.join(name)).unwrap()
}

#[test]
fn vault_roundtrip_create_lock_unlock() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut vault = create_vault(a.clone(), MASTER_A);
    populate(&mut vault);
    let store_before = vault.store.clone();
    vault.lock();

    let unlocked = Vault::unlock(a, &secret(MASTER_A)).unwrap();
    assert_eq!(unlocked.store.vault_id, store_before.vault_id);
    assert_eq!(unlocked.store.connections.len(), 3);
    assert_eq!(unlocked.store.credentials.len(), 2);
    assert_secrets_of(&unlocked, &uid(101), "pw-secret-1", "pp-secret-1");
    assert_eq!(
        unlocked
            .secret(&uid(301), SecretField::MacroContent)
            .unwrap()
            .expose_secret(),
        "systemctl restart nginx"
    );
}

/// Critère n°1 : export profil A → import profil B vierge (autre répertoire,
/// autre mot de passe maître) → tous les secrets retrouvés sans ressaisie.
#[test]
fn export_a_import_b_virgin_different_master() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let b = dd(tmp.path(), "b");
    let mut va = create_vault(a.clone(), MASTER_A);
    populate(&mut va);
    let bundle = tmp.path().join("bundle.hrkx");
    let counts = export::export_bundle(
        &va,
        &ExportScope::All,
        &secret(EXPORT_PASS),
        Some(export_kdf()),
        &bundle,
    )
    .unwrap();
    assert_eq!(counts.connections, 3);
    assert_eq!(counts.credentials, 2);
    va.lock();

    let mut vb = create_vault(b.clone(), MASTER_B);
    assert!(vb.store.connections.is_empty());
    let preview = export::preview(&bundle, &secret(EXPORT_PASS), &vb.store).unwrap();
    assert!(preview.conflicts.is_empty());
    let report = export::apply(&mut vb, preview, Strategy::Replace, &HashMap::new()).unwrap();
    assert_eq!(report.connections, 3);
    vb.lock();

    let vb = Vault::unlock(b, &secret(MASTER_B)).unwrap();
    assert_secrets_of(&vb, &uid(101), "pw-secret-1", "pp-secret-1");
    assert_eq!(
        vb.secret(&uid(102), SecretField::Password)
            .unwrap()
            .expose_secret(),
        "pw-secret-2"
    );
    let conn = vb
        .store
        .connections
        .iter()
        .find(|c| c.id == uid(201))
        .unwrap();
    assert_eq!(
        effective::credential_for(&vb.store, conn).as_deref(),
        Some(uid(101).as_str())
    );
    assert!(!vb.store.settings.keyring_cache); // réglages locaux non importés
}

fn sample_offsets(len: usize) -> Vec<usize> {
    let mut offsets: Vec<usize> = (0..len.min(160)).collect();
    offsets.extend((160..len).step_by(61));
    offsets.extend(len.saturating_sub(16)..len);
    offsets.sort_unstable();
    offsets.dedup();
    offsets
}

/// Falsification : tout bit modifié dans le fichier coffre → rejet.
#[test]
fn vault_bitflip_anywhere_rejected() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut vault = create_vault(a.clone(), MASTER_A);
    populate(&mut vault);
    vault.lock();
    let original = fs::read(a.vault_file()).unwrap();
    for off in sample_offsets(original.len()) {
        let mut corrupted = original.clone();
        corrupted[off] ^= 0x01;
        fs::write(a.vault_file(), &corrupted).unwrap();
        let result = Vault::unlock(a.clone(), &secret(MASTER_A));
        assert!(result.is_err(), "bit flip à l'offset {off} accepté à tort");
    }
    fs::write(a.vault_file(), &original).unwrap();
    assert!(Vault::unlock(a, &secret(MASTER_A)).is_ok());
}

/// Falsification : tout bit modifié dans le bundle d'export → rejet.
#[test]
fn export_bitflip_anywhere_rejected() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut vault = create_vault(a.clone(), MASTER_A);
    populate(&mut vault);
    let bundle = tmp.path().join("bundle.hrkx");
    export::export_bundle(
        &vault,
        &ExportScope::All,
        &secret(EXPORT_PASS),
        Some(export_kdf()),
        &bundle,
    )
    .unwrap();
    let local = haraka_core::Store::new("local".into());
    let original = fs::read(&bundle).unwrap();
    for off in sample_offsets(original.len()) {
        let mut corrupted = original.clone();
        corrupted[off] ^= 0x01;
        fs::write(&bundle, &corrupted).unwrap();
        let result = export::preview(&bundle, &secret(EXPORT_PASS), &local);
        assert!(result.is_err(), "bit flip export à l'offset {off} accepté");
    }
}

/// Permutation de secrets entre entrées → échec AAD → rejet à l'ouverture.
#[test]
fn secret_swap_between_entries_rejected() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut vault = create_vault(a.clone(), MASTER_A);
    populate(&mut vault);
    let p1 = vault.store.credentials[0].password.clone();
    let p2 = vault.store.credentials[1].password.clone();
    vault.store.credentials[0].password = p2;
    vault.store.credentials[1].password = p1;
    vault.save().unwrap();
    vault.lock();
    assert!(matches!(
        Vault::unlock(a, &secret(MASTER_A)),
        Err(Error::Tampered)
    ));
}

/// Permutation entre champs (password ↔ passphrase) → échec AAD → rejet.
#[test]
fn secret_swap_between_fields_rejected() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut vault = create_vault(a.clone(), MASTER_A);
    populate(&mut vault);
    let c = &mut vault.store.credentials[0];
    std::mem::swap(&mut c.password, &mut c.passphrase);
    vault.save().unwrap();
    vault.lock();
    assert!(matches!(
        Vault::unlock(a, &secret(MASTER_A)),
        Err(Error::Tampered)
    ));
}

#[test]
fn wrong_password_and_passphrase_rejected() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut vault = create_vault(a.clone(), MASTER_A);
    populate(&mut vault);
    let bundle = tmp.path().join("bundle.hrkx");
    export::export_bundle(
        &vault,
        &ExportScope::All,
        &secret(EXPORT_PASS),
        Some(export_kdf()),
        &bundle,
    )
    .unwrap();
    vault.lock();
    assert!(matches!(
        Vault::unlock(a, &secret("mauvais-master")),
        Err(Error::WrongPassword)
    ));
    let local = haraka_core::Store::new("local".into());
    assert!(matches!(
        export::preview(&bundle, &secret("mauvaise-passphrase"), &local),
        Err(Error::WrongPassword)
    ));
}

/// Changement de master : re-wrap de la DEK uniquement, payload inchangé.
#[test]
fn master_change_rewraps_dek_only() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut vault = create_vault(a.clone(), MASTER_A);
    populate(&mut vault);
    let before = Envelope::decode(&fs::read(a.vault_file()).unwrap()).unwrap();
    vault
        .change_master(&secret(MASTER_A), &secret("nouveau-master"))
        .unwrap();
    let after = Envelope::decode(&fs::read(a.vault_file()).unwrap()).unwrap();
    assert_eq!(before.header, after.header);
    assert_eq!(before.payload_nonce, after.payload_nonce);
    assert_eq!(before.payload_ct, after.payload_ct);
    assert_ne!(before.kdf, after.kdf);
    assert_ne!(before.wrapped_dek, after.wrapped_dek);
    vault.lock();
    assert!(Vault::unlock(a.clone(), &secret(MASTER_A)).is_err());
    let vault = Vault::unlock(a, &secret("nouveau-master")).unwrap();
    assert_secrets_of(&vault, &uid(101), "pw-secret-1", "pp-secret-1");
}

#[test]
fn export_no_secrets_marks_reentry() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let b = dd(tmp.path(), "b");
    let mut va = create_vault(a.clone(), MASTER_A);
    populate(&mut va);
    let bundle = tmp.path().join("nosecrets.hrkx");
    export::export_bundle(
        &va,
        &ExportScope::NoSecrets,
        &secret(EXPORT_PASS),
        Some(export_kdf()),
        &bundle,
    )
    .unwrap();
    let mut vb = create_vault(b.clone(), MASTER_B);
    let preview = export::preview(&bundle, &secret(EXPORT_PASS), &vb.store).unwrap();
    assert!(!preview.header.content.includes_secrets);
    for c in &preview.payload.credentials {
        assert!(c.password.is_none() && c.private_key.is_none() && c.passphrase.is_none());
        assert!(!c.needs_reentry.is_empty());
    }
    assert!(preview.payload.macros.is_empty());
    export::apply(&mut vb, preview, Strategy::Replace, &HashMap::new()).unwrap();
    assert!(vb.secret(&uid(101), SecretField::Password).is_err());
    assert!(
        vb.store
            .credential(&uid(101))
            .unwrap()
            .needs_reentry
            .contains(&"password".to_string())
    );
}

#[test]
fn export_folder_scope_subtree_and_referenced_credentials() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut va = create_vault(a.clone(), MASTER_A);
    populate(&mut va);
    let bundle = tmp.path().join("folder.hrkx");
    export::export_bundle(
        &va,
        &ExportScope::Folder(uid(2)),
        &secret(EXPORT_PASS),
        Some(export_kdf()),
        &bundle,
    )
    .unwrap();
    let local = haraka_core::Store::new("local".into());
    let preview = export::preview(&bundle, &secret(EXPORT_PASS), &local).unwrap();
    assert_eq!(preview.payload.folders.len(), 1);
    assert_eq!(preview.payload.folders[0].id, uid(2));
    assert_eq!(preview.payload.folders[0].parent_id, None); // racine exportée
    assert_eq!(preview.payload.connections.len(), 1);
    assert_eq!(preview.payload.connections[0].id, uid(201));
    // cred1 référencé par héritage depuis f1 (hors périmètre) : inclus quand même.
    assert_eq!(preview.payload.credentials.len(), 1);
    assert_eq!(preview.payload.credentials[0].id, uid(101));
    assert_eq!(preview.payload.macros.len(), 1);
}

#[test]
fn import_conflicts_keep_replace_duplicate() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let b = dd(tmp.path(), "b");
    let mut va = create_vault(a.clone(), MASTER_A);
    populate(&mut va);
    let bundle = tmp.path().join("bundle.hrkx");
    export::export_bundle(
        &va,
        &ExportScope::All,
        &secret(EXPORT_PASS),
        Some(export_kdf()),
        &bundle,
    )
    .unwrap();
    let mut vb = create_vault(b.clone(), MASTER_B);
    let preview = export::preview(&bundle, &secret(EXPORT_PASS), &vb.store).unwrap();
    export::apply(&mut vb, preview, Strategy::Replace, &HashMap::new()).unwrap();

    // Keep : tout existe déjà → rien n'est modifié.
    let preview = export::preview(&bundle, &secret(EXPORT_PASS), &vb.store).unwrap();
    assert_eq!(preview.conflicts.len(), 3 + 3 + 2 + 1);
    let report = export::apply(&mut vb, preview, Strategy::Keep, &HashMap::new()).unwrap();
    assert_eq!(report.connections, 0);
    assert_eq!(vb.store.connections.len(), 3);

    // Replace : mêmes UUID, valeurs remplacées.
    let preview = export::preview(&bundle, &secret(EXPORT_PASS), &vb.store).unwrap();
    let report = export::apply(&mut vb, preview, Strategy::Replace, &HashMap::new()).unwrap();
    assert_eq!(report.connections, 3);
    assert_eq!(vb.store.connections.len(), 3);

    // Duplicate : nouveaux UUID, références remappées, secrets toujours lisibles.
    let preview = export::preview(&bundle, &secret(EXPORT_PASS), &vb.store).unwrap();
    let report = export::apply(&mut vb, preview, Strategy::Duplicate, &HashMap::new()).unwrap();
    assert_eq!(report.connections, 3);
    assert_eq!(vb.store.connections.len(), 6);
    assert_eq!(vb.store.credentials.len(), 4);
    let dup_conn = vb
        .store
        .connections
        .iter()
        .find(|c| c.name == "web-01" && c.id != uid(201))
        .unwrap();
    assert_ne!(dup_conn.folder_id, uid(2)); // dossier dupliqué aussi
    let dup_cred_id = effective::credential_for(&vb.store, dup_conn).unwrap();
    assert_ne!(dup_cred_id, uid(101));
    assert_eq!(
        vb.secret(&dup_cred_id, SecretField::Password)
            .unwrap()
            .expose_secret(),
        "pw-secret-1"
    );
    // L'original reste intact.
    assert_secrets_of(&vb, &uid(101), "pw-secret-1", "pp-secret-1");
}

#[test]
fn truncated_and_trailing_files_rejected() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut vault = create_vault(a.clone(), MASTER_A);
    populate(&mut vault);
    let bundle = tmp.path().join("bundle.hrkx");
    export::export_bundle(
        &vault,
        &ExportScope::All,
        &secret(EXPORT_PASS),
        Some(export_kdf()),
        &bundle,
    )
    .unwrap();
    let local = haraka_core::Store::new("local".into());
    let original = fs::read(&bundle).unwrap();
    for cut in [0, 8, 14, 40, original.len() / 2, original.len() - 1] {
        fs::write(&bundle, &original[..cut]).unwrap();
        assert!(export::preview(&bundle, &secret(EXPORT_PASS), &local).is_err());
    }
    let mut trailing = original.clone();
    trailing.extend_from_slice(b"surplus");
    fs::write(&bundle, &trailing).unwrap();
    assert!(export::preview(&bundle, &secret(EXPORT_PASS), &local).is_err());
}

#[test]
fn unknown_format_version_rejected() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let vault = create_vault(a.clone(), MASTER_A);
    let mut bytes = fs::read(a.vault_file()).unwrap();
    bytes[9] = 99;
    fs::write(a.vault_file(), &bytes).unwrap();
    drop(vault);
    assert!(matches!(
        Vault::unlock(a, &secret(MASTER_A)),
        Err(Error::UnsupportedVersion(_)) | Err(Error::BadFormat(_))
    ));
}

#[test]
fn nonces_unique_across_exports_and_secrets() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut vault = create_vault(a.clone(), MASTER_A);
    populate(&mut vault);
    let b1 = tmp.path().join("b1.hrkx");
    let b2 = tmp.path().join("b2.hrkx");
    for path in [&b1, &b2] {
        export::export_bundle(
            &vault,
            &ExportScope::All,
            &secret(EXPORT_PASS),
            Some(export_kdf()),
            path,
        )
        .unwrap();
    }
    let e1 = Envelope::decode(&fs::read(&b1).unwrap()).unwrap();
    let e2 = Envelope::decode(&fs::read(&b2).unwrap()).unwrap();
    assert_ne!(e1.payload_nonce, e2.payload_nonce);
    assert_ne!(e1.dek_nonce, e2.dek_nonce);
    let c = vault.store.credential(&uid(101)).unwrap();
    assert_ne!(
        c.password.as_ref().unwrap().nonce,
        c.passphrase.as_ref().unwrap().nonce
    );
}

#[cfg(unix)]
#[test]
fn permissions_0600_0700() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut vault = create_vault(a.clone(), MASTER_A);
    populate(&mut vault);
    let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(a.root()), 0o700);
    assert_eq!(mode(&a.vault_file()), 0o600);
    assert_eq!(mode(&a.backup_file(1)), 0o600);
    assert_eq!(mode(&a.audit_file()), 0o600);
    assert_eq!(mode(&a.tmp_dir()), 0o700);
}

/// Exigence utilisateur : sans Secret Service, rien n'est bloquant.
#[test]
fn keyring_absent_never_blocks() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let opts = haraka_core::vault::CreateOptions {
        keyring_cache: true,
        kdf: Some(KdfParams::insecure_test().unwrap()),
    };
    let mut vault = Vault::create(a.clone(), &secret(MASTER_A), opts).unwrap();
    populate(&mut vault);
    vault.lock();
    // Le cache a pu échouer silencieusement : le master fonctionne toujours.
    let vault = Vault::unlock(a, &secret(MASTER_A)).unwrap();
    assert_secrets_of(&vault, &uid(101), "pw-secret-1", "pp-secret-1");
}

/// Stabilité du format sur les fixtures versionnées (générées par `xtask fixtures`).
#[test]
fn versioned_fixtures_stable() {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let vault_path = fixtures.join("vault_v1.hrk");
    let export_path = fixtures.join("export_v1.hrkx");
    if !vault_path.exists() || !export_path.exists() {
        eprintln!("fixtures absentes — lancer `cargo run -p xtask -- fixtures`");
        return;
    }
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    fs::copy(&vault_path, a.vault_file()).unwrap();
    let vault = Vault::unlock(a, &secret(FIXTURE_MASTER)).unwrap();
    assert_eq!(vault.store.schema_version, 1);
    let local = haraka_core::Store::new("local".into());
    let preview = export::preview(&export_path, &secret(FIXTURE_EXPORT), &local).unwrap();
    assert_eq!(preview.header.content.scope, "all");
}

#[test]
fn envelope_magic_and_version_constants() {
    assert_eq!(&VAULT_MAGIC, b"HRKTSVL1");
    assert_eq!(FORMAT_VERSION, 1);
}

#[test]
fn backup_rotation_and_restore() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut vault = create_vault(a.clone(), MASTER_A);
    populate(&mut vault);
    for i in 0..7 {
        vault.store.settings.autolock_minutes = 10 + i;
        vault.save().unwrap();
    }
    assert!(a.backup_file(1).exists());
    assert!(a.backup_file(5).exists());
    assert!(!a.backup_file(6).exists());
    let corrupted = vec![0u8; 64];
    fs::write(a.vault_file(), &corrupted).unwrap();
    vault.lock();
    assert!(Vault::unlock(a.clone(), &secret(MASTER_A)).is_err());
    haraka_core::store::restore_backup(&a, 1).unwrap();
    let vault = Vault::unlock(a, &secret(MASTER_A)).unwrap();
    assert_eq!(vault.store.settings.autolock_minutes, 15);
}

#[test]
fn create_refuses_existing_vault() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let _vault = create_vault(a.clone(), MASTER_A);
    assert!(matches!(
        Vault::create(a, &secret(MASTER_A), test_opts()),
        Err(Error::AlreadyExists)
    ));
}

#[test]
fn wrong_magic_export_vs_vault() {
    let tmp = tempdir().unwrap();
    let a = dd(tmp.path(), "a");
    let mut vault = create_vault(a.clone(), MASTER_A);
    populate(&mut vault);
    let bundle = tmp.path().join("bundle.hrkx");
    export::export_bundle(
        &vault,
        &ExportScope::All,
        &secret(EXPORT_PASS),
        Some(export_kdf()),
        &bundle,
    )
    .unwrap();
    // Un bundle d'export ne s'ouvre pas comme un coffre.
    let b = dd(tmp.path(), "b");
    fs::copy(&bundle, b.vault_file()).unwrap();
    assert!(Vault::unlock(b, &secret(EXPORT_PASS)).is_err());
    // Et inversement.
    let local = haraka_core::Store::new("local".into());
    assert!(export::preview(&a.vault_file(), &secret(MASTER_A), &local).is_err());
}
