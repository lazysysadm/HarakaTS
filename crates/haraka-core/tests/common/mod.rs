//! Helpers partagés des tests normatifs (EXPORT_FORMAT.md §9).

use haraka_core::crypto::KdfParams;
use haraka_core::model::{Connection, ConnectionDefaults, Credential, Folder, Macro};
use haraka_core::store::DataDir;
use haraka_core::vault::{CreateOptions, SecretField, Vault};
use secrecy::SecretString;

pub const MASTER_A: &str = "master-A-test";
pub const MASTER_B: &str = "master-B-test";
pub const EXPORT_PASS: &str = "export-pass-test";
pub const FIXTURE_MASTER: &str = "harakats-fixture-master";
pub const FIXTURE_EXPORT: &str = "harakats-fixture-export";

pub fn uid(n: u64) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

pub fn secret(s: &str) -> SecretString {
    SecretString::from(s.to_string())
}

pub fn test_opts() -> CreateOptions {
    CreateOptions {
        keyring_cache: false,
        kdf: Some(KdfParams::insecure_test().unwrap()),
    }
}

pub fn export_kdf() -> KdfParams {
    KdfParams::insecure_test().unwrap()
}

pub fn create_vault(dd: DataDir, master: &str) -> Vault {
    Vault::create(dd, &secret(master), test_opts()).unwrap()
}

/// Store d'exemple : 2 dossiers imbriqués + 1 dossier hors périmètre,
/// 3 connexions, 2 credentials (secrets complets), 1 macro.
pub fn populate(vault: &mut Vault) {
    let now = haraka_core::crypto::now_rfc3339();
    let f1 = uid(1);
    let f2 = uid(2);
    let f3 = uid(3);
    let cred1 = uid(101);
    let cred2 = uid(102);
    let c1 = uid(201);
    let c2 = uid(202);
    let c3 = uid(203);
    let m1 = uid(301);

    vault.store.folders = vec![
        Folder {
            id: f1.clone(),
            parent_id: None,
            name: "Prod".into(),
            color: Some("#ff0000".into()),
            icon: None,
            sort_order: 0,
            tags: vec!["prod".into()],
            defaults: ConnectionDefaults {
                user: Some("deploy".into()),
                port: Some(2222),
                credential_id: Some(cred1.clone()),
                ..Default::default()
            },
        },
        Folder {
            id: f2.clone(),
            parent_id: Some(f1.clone()),
            name: "Web".into(),
            color: None,
            icon: None,
            sort_order: 0,
            tags: vec![],
            defaults: ConnectionDefaults::default(),
        },
        Folder {
            id: f3.clone(),
            parent_id: None,
            name: "Hors périmètre".into(),
            color: None,
            icon: None,
            sort_order: 1,
            tags: vec![],
            defaults: ConnectionDefaults {
                credential_id: Some(cred2.clone()),
                ..Default::default()
            },
        },
    ];
    vault.store.credentials = vec![
        Credential {
            id: cred1.clone(),
            name: "deploy-prod".into(),
            username: "deploy".into(),
            password: None,
            private_key: None,
            passphrase: None,
            key_origin: Some("pasted".into()),
            needs_reentry: vec![],
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        Credential {
            id: cred2.clone(),
            name: "autre".into(),
            username: "admin".into(),
            password: None,
            private_key: None,
            passphrase: None,
            key_origin: None,
            needs_reentry: vec![],
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    ];
    vault.store.connections = vec![
        Connection {
            id: c1.clone(),
            folder_id: f2.clone(),
            name: "web-01".into(),
            protocol: "ssh".into(),
            host: "web-01.example.net".into(),
            port: None,
            user: None,
            credential_id: None,
            jump_chain: None,
            clipboard_policy: None,
            keepalive_s: None,
            encoding: None,
            color: None,
            icon: None,
            tags: vec![],
            favorite: true,
            tunnels: vec![],
            startup_snippet_id: Some(m1.clone()),
            session_logging: false,
            sftp: Default::default(),
            notes: String::new(),
            created_at: now.clone(),
            updated_at: now.clone(),
            last_used_at: None,
        },
        Connection {
            id: c2.clone(),
            folder_id: f1.clone(),
            name: "db-01".into(),
            protocol: "ssh".into(),
            host: "db-01.example.net".into(),
            port: Some(22),
            user: Some("postgres".into()),
            credential_id: None,
            jump_chain: None,
            clipboard_policy: None,
            keepalive_s: None,
            encoding: None,
            color: None,
            icon: None,
            tags: vec![],
            favorite: false,
            tunnels: vec![],
            startup_snippet_id: None,
            session_logging: false,
            sftp: Default::default(),
            notes: String::new(),
            created_at: now.clone(),
            updated_at: now.clone(),
            last_used_at: None,
        },
        Connection {
            id: c3.clone(),
            folder_id: f3.clone(),
            name: "hors-sujet".into(),
            protocol: "ssh".into(),
            host: "autre.example.net".into(),
            port: None,
            user: None,
            credential_id: None,
            jump_chain: None,
            clipboard_policy: None,
            keepalive_s: None,
            encoding: None,
            color: None,
            icon: None,
            tags: vec![],
            favorite: false,
            tunnels: vec![],
            startup_snippet_id: None,
            session_logging: false,
            sftp: Default::default(),
            notes: String::new(),
            created_at: now.clone(),
            updated_at: now.clone(),
            last_used_at: None,
        },
    ];
    vault.store.macros = vec![Macro {
        id: m1.clone(),
        name: "restart-nginx".into(),
        content: haraka_core::model::EncSecret::new(String::new(), String::new()),
        confirm_before_send: true,
    }];

    vault
        .set_secret(&cred1, SecretField::Password, &secret("pw-secret-1"))
        .unwrap();
    vault
        .set_secret(
            &cred1,
            SecretField::PrivateKey,
            &secret("-----BEGIN OPENSSH PRIVATE KEY-----\nfixture-key\n-----END OPENSSH PRIVATE KEY-----"),
        )
        .unwrap();
    vault
        .set_secret(&cred1, SecretField::Passphrase, &secret("pp-secret-1"))
        .unwrap();
    vault
        .set_secret(&cred2, SecretField::Password, &secret("pw-secret-2"))
        .unwrap();
    vault
        .set_secret(
            &m1,
            SecretField::MacroContent,
            &secret("systemctl restart nginx"),
        )
        .unwrap();
    vault.save().unwrap();
}

pub fn assert_secrets_of(vault: &Vault, cred_id: &str, pw: &str, pp: &str) {
    assert_eq!(
        vault
            .secret(cred_id, SecretField::Password)
            .unwrap()
            .expose_secret(),
        pw
    );
    assert_eq!(
        vault
            .secret(cred_id, SecretField::Passphrase)
            .unwrap()
            .expose_secret(),
        pp
    );
}

use secrecy::ExposeSecret;
