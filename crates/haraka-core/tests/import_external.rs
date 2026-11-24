//! Imports externes (M7) : ssh_config, Remmina, MobaXterm. Fixtures anonymisées
//! sous `tests/fixtures/import/`. Règle testée : jamais de panique, rapport seul.

#[allow(dead_code)]
mod common;

use std::collections::HashMap;
use std::path::PathBuf;

use haraka_core::import::ssh_config::{self, FsIncludes, IncludeProvider, NoIncludes};
use haraka_core::import::{
    self, ImportPlan, KeyMaterial, apply, apply_to_store, decode_text, mobaxterm, remmina,
    summarize,
};
use haraka_core::manage::validate_store;
use haraka_core::{Store, vault::SecretField};
use secrecy::{ExposeSecret, SecretString};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/import")
}

fn read(rel: &str) -> String {
    let bytes = std::fs::read(fixtures().join(rel)).unwrap();
    decode_text(&bytes).0
}

const HOME: &str = "/home/tester";

fn ssh_plan() -> ImportPlan {
    let dir = fixtures().join("ssh");
    let mut prov = FsIncludes {
        ssh_dir: dir.display().to_string(),
    };
    ssh_config::parse(
        &read("ssh/config"),
        HOME,
        &dir.display().to_string(),
        &mut prov,
    )
}

fn conn<'a>(p: &'a ImportPlan, name: &str) -> &'a import::PlanConnection {
    p.connections
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("connexion {name} absente"))
}

fn warn_codes(p: &ImportPlan) -> Vec<&str> {
    p.warnings.iter().map(|w| w.code.as_str()).collect()
}

#[test]
fn ssh_config_hosts_defaults_and_includes() {
    let p = ssh_plan();
    let names: Vec<&str> = p.connections.iter().map(|c| c.name.as_str()).collect();
    for n in [
        "work-vpn",
        "build",
        "cyc",
        "bastion",
        "web1",
        "web2",
        "db1",
        "lab-a.lab",
        "lab-secret.lab",
        "legacy",
        "quoted",
        "sneaky",
    ] {
        assert!(names.contains(&n), "{n} manquant dans {names:?}");
    }
    assert!(!names.iter().any(|n| n.contains('*')));
    // Défauts de dossier issus de `Host *`.
    assert_eq!(p.root_defaults.user.as_deref(), Some("deploy"));
    assert_eq!(p.root_defaults.keepalive_s, Some(30));
    assert_eq!(
        p.root_defaults.credential.as_deref(),
        Some("/home/tester/.ssh/id_ed25519")
    );
    // Héritage : web1 ne répète pas user/keepalive/credential.
    let web1 = conn(&p, "web1");
    assert_eq!(web1.host, "web1.prod.example.net");
    assert_eq!(conn(&p, "web2").host, "web2.prod.example.net");
    assert_eq!(web1.user, None);
    assert_eq!(web1.credential, None);
    assert_eq!(web1.keepalive_s, None);
    // Surcharges.
    let bastion = conn(&p, "bastion");
    assert_eq!(bastion.port, Some(2222));
    assert_eq!(bastion.user.as_deref(), Some("jump"));
    assert_eq!(
        bastion.credential.as_deref(),
        Some("/home/tester/.ssh/id_bastion")
    );
    // Include relatif.
    assert_eq!(conn(&p, "work-vpn").user.as_deref(), Some("alice"));
    assert_eq!(conn(&p, "work-vpn").port, Some(443));
    // Motif + négation.
    assert_eq!(conn(&p, "lab-a.lab").user.as_deref(), Some("labuser"));
    assert_eq!(conn(&p, "lab-secret.lab").user, None);
    // Guillemets.
    let q = conn(&p, "quoted");
    assert_eq!(q.host, "quoted host.example.net");
    assert_eq!(
        q.credential.as_deref(),
        Some("/home/tester/.ssh/id with space")
    );
}

#[test]
fn ssh_config_proxyjump_chain_and_forwards() {
    let p = ssh_plan();
    let web1 = conn(&p, "web1");
    let hops = web1.jump_chain.as_ref().unwrap();
    assert_eq!(hops.len(), 1);
    // Alias « bastion » résolu.
    assert_eq!(hops[0].host, "bastion.example.net");
    assert_eq!(hops[0].port, Some(2222));
    assert_eq!(hops[0].user.as_deref(), Some("jump"));
    let db = conn(&p, "db1");
    let chain = db.jump_chain.as_ref().unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].host, "gw1.example.net");
    assert_eq!(chain[0].port, Some(2200));
    assert_eq!(chain[0].user.as_deref(), Some("jump1"));
    assert_eq!(chain[1].host, "bastion.example.net");
    assert_eq!(db.port, Some(2201));
    assert_eq!(conn(&p, "sneaky").jump_chain, Some(vec![]));
    let kinds: Vec<&str> = web1.tunnels.iter().map(|t| t.kind.as_str()).collect();
    assert_eq!(kinds, ["local", "local", "remote", "dynamic"]);
    assert_eq!(web1.tunnels[0].bind_port, 8080);
    assert_eq!(web1.tunnels[0].dest_host.as_deref(), Some("localhost"));
    assert_eq!(web1.tunnels[1].dest_port, Some(5432));
    assert_eq!(web1.tunnels[3].bind_port, 1080);
}

#[test]
fn ssh_config_unsupported_are_reported_not_silent() {
    let p = ssh_plan();
    let codes = warn_codes(&p);
    assert!(codes.contains(&"ssh_match_unsupported"));
    assert!(codes.contains(&"ssh_proxycommand"));
    assert!(codes.contains(&"ssh_include_cycle"));
    assert!(codes.contains(&"ssh_invalid_port"));
    assert!(codes.contains(&"ssh_options_ignored"));
    assert!(codes.contains(&"ssh_extra_identities"));
    assert!(codes.contains(&"ssh_wildcard_unused"));
    // Rien de ce qui est derrière Match n'a fuité.
    assert!(
        !p.connections
            .iter()
            .any(|c| c.host == "ignored.example.net")
    );
    assert!(p.skipped >= 2);
    let opts = p
        .warnings
        .iter()
        .find(|w| w.code == "ssh_options_ignored")
        .unwrap();
    assert!(opts.params["options"].contains("compression"));
    // Un seul credential par fichier de clé.
    let mut keys: Vec<_> = p.credentials.iter().map(|c| &c.key).collect();
    let n = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(n, keys.len());
}

#[test]
fn ssh_config_in_memory_include_provider() {
    struct Mem(HashMap<String, String>);
    impl IncludeProvider for Mem {
        fn expand(&mut self, pattern: &str) -> Vec<(String, String)> {
            self.0
                .get(pattern)
                .map(|c| vec![(pattern.to_string(), c.clone())])
                .unwrap_or_default()
        }
    }
    let mut m = HashMap::new();
    m.insert(
        "/x/a".to_string(),
        "Host a\n HostName a.example\nInclude /x/a\n".to_string(),
    );
    let p = ssh_config::parse("Include /x/a\nHost b\n", HOME, "/x", &mut Mem(m));
    assert_eq!(p.connections.len(), 2);
    assert!(warn_codes(&p).contains(&"ssh_include_cycle"));
}

#[test]
fn glob_and_expand() {
    use haraka_core::import::ssh_config::{expand_path, glob_match};
    assert!(glob_match("*.example.com", "a.Example.com", true));
    assert!(!glob_match("a?c", "abbc", false));
    assert!(glob_match("a*b*c", "aXXbYYc", false));
    assert_eq!(expand_path("~/k", "/h"), "/h/k");
    assert_eq!(expand_path("%d/.ssh/k%%", "/h"), "/h/.ssh/k%");
}

#[test]
fn remmina_files() {
    let names = [
        "ssh-prod.remmina",
        "ssh-tunnel.remmina",
        "rdp.remmina",
        "vnc.remmina",
        "malformed.remmina",
        "empty.remmina",
        "no-server.remmina",
    ];
    let files: Vec<(String, String)> = names
        .iter()
        .map(|n| ((*n).to_string(), read(&format!("remmina/{n}"))))
        .collect();
    let p = remmina::parse(&files, HOME);
    assert_eq!(p.connections.len(), 2, "{:?}", p.warnings);
    let prod = conn(&p, "Prod web");
    assert_eq!(prod.host, "web.prod.example.net");
    assert_eq!(prod.port, Some(2222));
    assert_eq!(prod.user.as_deref(), Some("ops"));
    assert_eq!(prod.folder_path, ["Production", "Web"]);
    let cred = p
        .credentials
        .iter()
        .find(|c| Some(&c.key) == prod.credential.as_ref())
        .unwrap();
    assert!(cred.password_needs_reentry);
    assert_eq!(cred.key_file.as_deref(), Some("/home/tester/.ssh/id_prod"));
    let tun = conn(&p, "Internal via gateway");
    let hop = &tun.jump_chain.as_ref().unwrap()[0];
    assert_eq!(
        (hop.host.as_str(), hop.port),
        ("gw.example.net", Some(2200))
    );
    assert_eq!(hop.user.as_deref(), Some("gwuser"));
    let codes = warn_codes(&p);
    assert_eq!(codes.iter().filter(|c| **c == "remmina_non_ssh").count(), 2);
    assert!(codes.contains(&"remmina_no_server"));
    assert!(p.skipped >= 4);
    assert!(
        p.folders
            .contains(&vec!["Production".to_string(), "Web".to_string()])
    );
}

#[test]
fn mobaxterm_sessions() {
    let p = mobaxterm::parse(&read("mobaxterm/sessions.mxtsessions"));
    let names: Vec<&str> = p.connections.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["Top server", "web 1", "web 1", "db"],
        "{:?}",
        p.warnings
    );
    let top = conn(&p, "Top server");
    assert!(top.folder_path.is_empty());
    assert_eq!(top.user.as_deref(), Some("root"));
    let web = p
        .connections
        .iter()
        .find(|c| c.host == "web1.example.net")
        .unwrap();
    assert_eq!(web.folder_path, ["Production", "Web"]);
    assert_eq!(web.port, Some(2222));
    assert_eq!(web.user, None, "<default> ignoré");
    assert!(p.folders.contains(&vec!["Production".to_string()]));
    assert!(
        p.folders
            .contains(&vec!["Production".to_string(), "DB".to_string()])
    );
    let codes = warn_codes(&p);
    assert!(codes.contains(&"mxt_non_ssh"));
    assert!(codes.contains(&"mxt_no_host"));
    assert!(codes.contains(&"mxt_malformed"));
    assert!(p.credentials.iter().all(|c| c.password_needs_reentry));
}

#[test]
fn decode_text_variants() {
    assert_eq!(decode_text(b"\xef\xbb\xbfabc").0, "abc");
    let utf16: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain("Host a\n".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    assert_eq!(decode_text(&utf16).0, "Host a\n");
    let (t, w) = decode_text(b"abc\0def");
    assert!(t.is_empty() && w[0].code == "binary_input");
    let (_, w) = decode_text(b"ab\xffcd");
    assert_eq!(w[0].code, "encoding_lossy");
}

#[test]
fn apply_creates_folder_credentials_and_keeps_existing() {
    let p = ssh_plan();
    let mut store = Store::new("v".into());
    // Dossier existant au même nom : le nouveau est suffixé, l'ancien intact.
    haraka_core::manage::upsert_folder(
        &mut store,
        haraka_core::model::Folder {
            id: "keep".into(),
            parent_id: None,
            name: "Import ssh_config".into(),
            color: None,
            icon: None,
            sort_order: 0,
            tags: vec![],
            defaults: Default::default(),
        },
    )
    .unwrap();
    let before = store.clone();
    let mut keys = HashMap::new();
    keys.insert(
        "/home/tester/.ssh/id_ed25519".to_string(),
        KeyMaterial {
            pem: SecretString::from("PEM-A".to_string()),
            encrypted: false,
        },
    );
    keys.insert(
        "/home/tester/.ssh/id_bastion".to_string(),
        KeyMaterial {
            pem: SecretString::from("PEM-B".to_string()),
            encrypted: true,
        },
    );
    let (report, secrets) = apply_to_store(&mut store, &p, &keys).unwrap();
    validate_store(&store).unwrap();
    assert_eq!(report.root_folder, "Import ssh_config (2)");
    assert_eq!(
        report.imported,
        p.connections.len() - 1,
        "hôte avec espace rejeté"
    );
    assert!(report.warnings.iter().any(|w| w.code == "invalid_host"));
    assert_eq!(store.folders[0], before.folders[0]);
    assert_eq!(secrets.len(), 2);
    // Défauts du dossier racine → credential importé.
    let root = store
        .folders
        .iter()
        .find(|f| f.name == report.root_folder)
        .unwrap();
    assert_eq!(root.defaults.user.as_deref(), Some("deploy"));
    assert!(root.defaults.credential_id.is_some());
    let bastion_cred = store
        .credentials
        .iter()
        .find(|c| c.name.starts_with("id_bastion"))
        .unwrap();
    assert_eq!(bastion_cred.needs_reentry, ["passphrase"]);
    // Clés non lues → private_key à ressaisir.
    let work = store
        .credentials
        .iter()
        .find(|c| c.name.starts_with("id_work "))
        .unwrap();
    assert_eq!(work.needs_reentry, ["private_key"]);
    assert!(report.secrets_to_reenter >= 2);
    // Tunnels convertis.
    let web1 = store.connections.iter().find(|c| c.name == "web1").unwrap();
    assert_eq!(web1.tunnels.len(), 4);
    // Résumé cohérent.
    let s = summarize(&p, &keys);
    assert_eq!(s.connections.len(), p.connections.len());
    assert!(s.keys.iter().any(|k| k.status == "encrypted"));
    assert!(s.keys.iter().any(|k| k.status == "unreadable"));
    assert_eq!(s.secrets_to_reenter, report.secrets_to_reenter);
}

#[test]
fn apply_names_collide_are_suffixed_and_secrets_encrypted() {
    use tempfile::tempdir;
    let p = mobaxterm::parse(&read("mobaxterm/sessions.mxtsessions"));
    let tmp = tempdir().unwrap();
    let dd = haraka_core::DataDir::open(tmp.path().join("d")).unwrap();
    let mut vault = common::create_vault(dd, common::MASTER_A);
    let report = apply(&mut vault, &p, &HashMap::new()).unwrap();
    assert_eq!(report.imported, 4);
    let names: Vec<&str> = vault
        .store
        .connections
        .iter()
        .filter(|c| c.host.starts_with("web1"))
        .map(|c| c.name.as_str())
        .collect();
    assert!(
        names.contains(&"web 1") && names.contains(&"web 1 (2)"),
        "{names:?}"
    );
    assert!(report.secrets_to_reenter >= 2);

    // Clé du coffre : chiffrée et relisible.
    let mut sp = ImportPlan::new(import::ImportSource::SshConfig);
    sp.credentials.push(import::PlanCredential {
        key: "k".into(),
        name: "k".into(),
        username: "u".into(),
        key_file: Some("/p".into()),
        password_needs_reentry: false,
    });
    let mut keys = HashMap::new();
    keys.insert(
        "/p".to_string(),
        KeyMaterial {
            pem: SecretString::from("PEM-DATA".to_string()),
            encrypted: false,
        },
    );
    apply(&mut vault, &sp, &keys).unwrap();
    let cred = vault
        .store
        .credentials
        .iter()
        .find(|c| c.name == "k")
        .unwrap()
        .clone();
    assert!(cred.needs_reentry.is_empty());
    assert_eq!(
        vault
            .secret(&cred.id, SecretField::PrivateKey)
            .unwrap()
            .expose_secret(),
        "PEM-DATA"
    );
    vault.save().unwrap();
}

// ------------------------------------------------------------ hostile / fuzz

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn all_parsers(bytes: &[u8]) {
    let (text, _) = decode_text(bytes);
    let dir = fixtures().join("ssh");
    let mut prov = FsIncludes {
        ssh_dir: dir.display().to_string(),
    };
    let plans = [
        ssh_config::parse(&text, HOME, "/nonexistent", &mut NoIncludes),
        ssh_config::parse(&text, HOME, &dir.display().to_string(), &mut prov),
        remmina::parse(&[("f.remmina".into(), text.clone())], HOME),
        mobaxterm::parse(&text),
    ];
    for plan in plans {
        let mut store = Store::new("v".into());
        let _ = summarize(&plan, &HashMap::new());
        let _ = apply_to_store(&mut store, &plan, &HashMap::new());
    }
}

#[test]
fn hostile_inputs_never_panic() {
    all_parsers(&std::fs::read(fixtures().join("hostile/garbage.bin")).unwrap());
    all_parsers(b"");
    all_parsers(&vec![b'a'; 3_000_000]);
    all_parsers(
        format!(
            "Host {}\n HostName {}",
            "x".repeat(100_000),
            "y".repeat(100_000)
        )
        .as_bytes(),
    );
    all_parsers(&b"Host a\n".repeat(50_000));
    all_parsers("Host é€😀 \u{0}\n".as_bytes());
    all_parsers(b"[Bookmarks]\nSubRep=\\\\\\\\\nx=#109#0%%%%\ny=#\nz=##\nw=#99999999999#\n");
    all_parsers(b"[remmina]\nserver=[::1\nprotocol=ssh\nssh_tunnel_enabled=1\n");
    all_parsers(
        b"Host a\nProxyJump ,,,@,@:\nLocalForward\nLocalForward x\nDynamicForward ::\nPort 99999\n",
    );
    // Dossiers très profonds.
    let deep = (0..200)
        .map(|i| format!("d{i}"))
        .collect::<Vec<_>>()
        .join("\\");
    all_parsers(format!("[Bookmarks]\nSubRep={deep}\ns=#109#0%h%22%u\n").as_bytes());
}

#[test]
fn fuzz_random_bytes_and_lines_never_panic() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let words: [&str; 22] = [
        "Host",
        "Match",
        "Include",
        "ProxyJump",
        "ProxyCommand",
        "LocalForward",
        "Port",
        "[remmina]",
        "[Bookmarks]",
        "protocol=SSH",
        "server=",
        "SubRep=",
        "#109#0%",
        "%",
        "=",
        "*",
        "!",
        "\"",
        "#",
        "\\",
        ",",
        ":",
    ];
    for _ in 0..400 {
        let mut buf: Vec<u8> = Vec::new();
        let lines = rng.next() % 30;
        for _ in 0..lines {
            for _ in 0..(rng.next() % 6) {
                match rng.next() % 3 {
                    0 => buf.extend_from_slice(words[(rng.next() % 22) as usize].as_bytes()),
                    1 => buf.push((rng.next() % 256) as u8),
                    _ => buf.extend_from_slice(b" "),
                }
            }
            buf.push(if rng.next().is_multiple_of(5) {
                b'\r'
            } else {
                b'\n'
            });
        }
        all_parsers(&buf);
    }
    for _ in 0..100 {
        let n = (rng.next() % 2000) as usize;
        let buf: Vec<u8> = (0..n).map(|_| (rng.next() % 256) as u8).collect();
        all_parsers(&buf);
    }
}
