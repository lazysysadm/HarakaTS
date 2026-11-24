//! xtask — tâches de build sans dépendance externe :
//! - `icons`    : génère les icônes PNG de src-tauri/icons (encodeur PNG minimal).
//! - `fixtures` : génère les fixtures versionnées du format v1 (passphrases
//!   publiques de test, paramètres KDF rapides — jamais de vrais secrets).
//! - `licenses` : génère THIRD-PARTY-LICENSES.md (cargo about + pnpm licenses).
//! - `all`      : icônes + fixtures (défaut).

use std::fs;
use std::path::Path;

use haraka_core::crypto::KdfParams;
use haraka_core::export::{self, ExportScope};
use haraka_core::model::{Connection, ConnectionDefaults, Credential, EncSecret, Folder, Macro};
use haraka_core::store::DataDir;
use haraka_core::vault::{CreateOptions, SecretField, Vault};
use secrecy::SecretString;

const FIXTURE_MASTER: &str = "harakats-fixture-master";
const FIXTURE_EXPORT: &str = "harakats-fixture-export";

fn main() {
    let task = std::env::args().nth(1).unwrap_or_else(|| "all".to_string());
    // `--force` : régénère les fixtures du format v1 (à NE PAS faire une fois le format figé).
    let force = std::env::args().any(|a| a == "--force");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("racine du workspace")
        .to_path_buf();
    let result = match task.as_str() {
        "icons" => icons(&root),
        "fixtures" => fixtures(&root, force),
        "version" => version(&root),
        "licenses" => licenses(&root),
        "all" => icons(&root).and_then(|_| fixtures(&root, force)),
        other => Err(format!(
            "tâche inconnue : {other} (icons|fixtures|licenses|version [check|sync|set X.Y.Z]|all [--force])"
        )),
    };
    if let Err(err) = result {
        eprintln!("xtask: {err}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------- icônes

fn icons(root: &Path) -> Result<(), String> {
    let dir = root.join("src-tauri/icons");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for (name, size) in [
        ("32x32.png", 32u32),
        ("128x128.png", 128),
        ("128x128@2x.png", 256),
        ("icon.png", 512),
    ] {
        let png = encode_png(size, size, &draw_h(size));
        fs::write(dir.join(name), png).map_err(|e| e.to_string())?;
        println!("icône {name} ({size}×{size})");
    }
    Ok(())
}

/// Pixels RGBA : fond sombre, lettre « H » accentuée (Tauri exige du RGBA).
fn draw_h(size: u32) -> Vec<u8> {
    let s = size as usize;
    let bg = [22u8, 22, 30];
    let fg = [137u8, 180, 250];
    let margin = s / 8;
    let stem = (s / 8).max(1);
    let mut px = vec![0u8; s * s * 4];
    for y in 0..s {
        for x in 0..s {
            let in_vertical = y >= margin && y < s - margin;
            let left = x >= margin && x < margin + stem;
            let right = x >= s - margin - stem && x < s - margin;
            let bar_y = s / 2 - stem / 2;
            let crossbar = y >= bar_y && y < bar_y + stem && x >= margin && x < s - margin;
            let color = if in_vertical && (left || right) || crossbar {
                fg
            } else {
                bg
            };
            let off = (y * s + x) * 4;
            px[off..off + 3].copy_from_slice(&color);
            px[off + 3] = 255;
        }
    }
    px
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut crc_input = tag.to_vec();
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    out
}

/// PNG RGBA 8 bits, sans compression (blocs deflate « stored ») : suffisant
/// pour des icônes unies, aucune dépendance externe.
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);

    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in rgba.chunks(stride) {
        raw.push(0u8); // filtre « none »
        raw.extend_from_slice(row);
    }

    let mut zlib = vec![0x78u8, 0x01];
    let mut rest = &raw[..];
    loop {
        let take = rest.len().min(65535);
        let final_block = take == rest.len();
        zlib.push(if final_block { 1 } else { 0 });
        zlib.extend_from_slice(&(take as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(take as u16)).to_le_bytes());
        zlib.extend_from_slice(&rest[..take]);
        rest = &rest[take..];
        if final_block {
            break;
        }
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&chunk(b"IDAT", &zlib));
    png.extend_from_slice(&chunk(b"IEND", &[]));
    png
}

// -------------------------------------------------------------- fixtures

/// Génère les fixtures du format v1. Le format étant FIGÉ (EXPORT_FORMAT.md), les fichiers
/// existants sont conservés : ils servent de référence de compatibilité (un coffre/export
/// v1 produit aujourd'hui doit rester lisible). `--force` les régénère explicitement.
fn fixtures(root: &Path, force: bool) -> Result<(), String> {
    let dir = root.join("tests/fixtures");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if !force && dir.join("vault_v1.hrk").exists() && dir.join("export_v1.hrkx").exists() {
        println!("fixtures : conservées (format v1 figé ; --force pour régénérer)");
        return Ok(());
    }
    let tmp = std::env::temp_dir().join(format!("harakats-xtask-{}", uuid::Uuid::new_v4()));
    let dd = DataDir::open(tmp.join("data")).map_err(|e| e.to_string())?;
    let opts = CreateOptions {
        keyring_cache: false,
        kdf: Some(KdfParams::insecure_test().map_err(|e| e.to_string())?),
    };
    let master = SecretString::from(FIXTURE_MASTER.to_string());
    let mut vault = Vault::create(dd.clone(), &master, opts).map_err(|e| e.to_string())?;

    let now = haraka_core::crypto::now_rfc3339();
    let fid = "00000000-0000-4000-8000-000000000001".to_string();
    let cid = "00000000-0000-4000-8000-000000000101".to_string();
    let nid = "00000000-0000-4000-8000-000000000201".to_string();
    let mid = "00000000-0000-4000-8000-000000000301".to_string();
    vault.store.folders = vec![Folder {
        id: fid.clone(),
        parent_id: None,
        name: "Fixtures".into(),
        color: None,
        icon: None,
        sort_order: 0,
        tags: vec![],
        defaults: ConnectionDefaults {
            credential_id: Some(cid.clone()),
            ..Default::default()
        },
    }];
    vault.store.credentials = vec![Credential {
        id: cid.clone(),
        name: "fixture-cred".into(),
        username: "fixture".into(),
        password: None,
        private_key: None,
        passphrase: None,
        key_origin: Some("pasted".into()),
        needs_reentry: vec![],
        created_at: now.clone(),
        updated_at: now.clone(),
    }];
    vault.store.connections = vec![Connection {
        id: nid.clone(),
        folder_id: fid.clone(),
        name: "fixture-host".into(),
        protocol: "ssh".into(),
        host: "fixture.example.net".into(),
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
        startup_snippet_id: Some(mid.clone()),
        session_logging: false,
        sftp: Default::default(),
        notes: String::new(),
        created_at: now.clone(),
        updated_at: now,
        last_used_at: None,
    }];
    vault.store.macros = vec![Macro {
        id: mid.clone(),
        name: "fixture-macro".into(),
        content: EncSecret::new(String::new(), String::new()),
        confirm_before_send: true,
    }];
    for (entry, field, value) in [
        (&cid, SecretField::Password, "fixture-password"),
        (
            &cid,
            SecretField::PrivateKey,
            "-----BEGIN OPENSSH PRIVATE KEY-----\nfixture\n-----END OPENSSH PRIVATE KEY-----",
        ),
        (&cid, SecretField::Passphrase, "fixture-passphrase"),
        (&mid, SecretField::MacroContent, "echo fixture"),
    ] {
        vault
            .set_secret(entry, field, &SecretString::from(value.to_string()))
            .map_err(|e| e.to_string())?;
    }
    vault.save().map_err(|e| e.to_string())?;
    fs::copy(dd.vault_file(), dir.join("vault_v1.hrk")).map_err(|e| e.to_string())?;

    let bundle = tmp.join("export_v1.hrkx");
    export::export_bundle(
        &vault,
        &ExportScope::All,
        &SecretString::from(FIXTURE_EXPORT.to_string()),
        Some(KdfParams::insecure_test().map_err(|e| e.to_string())?),
        &bundle,
    )
    .map_err(|e| e.to_string())?;
    fs::copy(&bundle, dir.join("export_v1.hrkx")).map_err(|e| e.to_string())?;
    vault.lock();
    let _ = fs::remove_dir_all(&tmp);
    println!("fixtures : tests/fixtures/vault_v1.hrk + export_v1.hrkx");
    println!("passphrases publiques de test : {FIXTURE_MASTER} / {FIXTURE_EXPORT}");
    Ok(())
}

// -------------------------------------------------------------- versions

/// Source unique : `[workspace.package] version` de `Cargo.toml`. Les fichiers JSON qui
/// doivent porter la même valeur (`package.json` ×2, `tauri.conf.json`) en sont dérivés.
/// `version check` (CI) échoue en cas de désaccord ; `version sync` les réaligne ;
/// `version set X.Y.Z` change la source puis synchronise.
const JSON_VERSION_FILES: [&str; 3] = [
    "package.json",
    "ui/package.json",
    "src-tauri/tauri.conf.json",
];

fn is_semver(v: &str) -> bool {
    let core = v.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Remplace la valeur de la première ligne `<prefix>"<ancienne>"` ; retourne l'ancienne.
fn replace_first_version(text: &str, prefix: &str, new: &str) -> Option<(String, String)> {
    let mut out = String::new();
    let mut old = None;
    for line in text.split_inclusive('\n') {
        if old.is_none() && line.trim_start().starts_with(prefix) {
            // La valeur suit le séparateur (`:` JSON, `=` TOML) : jamais la clé entre guillemets.
            let sep = line.find([':', '='])?;
            let start = sep + line[sep..].find('"')? + 1;
            let end = start + line[start..].find('"')?;
            old = Some(line[start..end].to_string());
            out.push_str(&format!("{}{}{}", &line[..start], new, &line[end..]));
        } else {
            out.push_str(line);
        }
    }
    old.map(|o| (out, o))
}

fn read_version(root: &Path, file: &str, prefix: &str) -> Result<String, String> {
    let text = fs::read_to_string(root.join(file)).map_err(|e| format!("{file} : {e}"))?;
    replace_first_version(&text, prefix, "")
        .map(|(_, old)| old)
        .ok_or_else(|| format!("{file} : version introuvable"))
}

fn version(root: &Path) -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let action = args.first().map(String::as_str).unwrap_or("check");
    let source = read_version(root, "Cargo.toml", "version = ")?;
    match action {
        "check" => {
            let mut bad = Vec::new();
            for f in JSON_VERSION_FILES {
                let v = read_version(root, f, "\"version\":")?;
                if v != source {
                    bad.push(format!("{f} = {v} (attendu {source})"));
                }
            }
            if bad.is_empty() {
                println!("versions alignées : {source}");
                Ok(())
            } else {
                Err(format!("versions désalignées : {}", bad.join(", ")))
            }
        }
        "sync" => sync_versions(root, &source),
        "set" => {
            let new = args.get(1).ok_or("usage : version set X.Y.Z")?;
            if !is_semver(new) {
                return Err(format!("version invalide : {new} (attendu X.Y.Z[-pre])"));
            }
            let path = root.join("Cargo.toml");
            let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let (out, _) = replace_first_version(&text, "version = ", new)
                .ok_or("Cargo.toml : version introuvable")?;
            fs::write(&path, out).map_err(|e| e.to_string())?;
            sync_versions(root, new)
        }
        other => Err(format!("action inconnue : {other} (check|sync|set X.Y.Z)")),
    }
}

fn sync_versions(root: &Path, v: &str) -> Result<(), String> {
    for f in JSON_VERSION_FILES {
        let path = root.join(f);
        let text = fs::read_to_string(&path).map_err(|e| format!("{f} : {e}"))?;
        let (out, old) = replace_first_version(&text, "\"version\":", v)
            .ok_or_else(|| format!("{f} : version introuvable"))?;
        if old != v {
            fs::write(&path, out).map_err(|e| format!("{f} : {e}"))?;
            println!("{f} : {old} -> {v}");
        }
    }
    println!("versions alignées : {v} (pensez à `cargo build` pour Cargo.lock)");
    Ok(())
}

// ---------------------------------------------------------------- licences

/// Génère THIRD-PARTY-LICENSES.md : avis Rust (cargo about, template about.hbs)
/// + avis npm (pnpm licenses list --json, textes LICENSE dédupliqués).
fn licenses(root: &Path) -> Result<(), String> {
    let about = std::process::Command::new("cargo")
        .args(["about", "generate", "about.hbs"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo about introuvable (cargo install cargo-about) : {e}"))?;
    if !about.status.success() {
        return Err(format!(
            "cargo about a échoué : {}",
            String::from_utf8_lossy(&about.stderr)
        ));
    }
    let rust_part = String::from_utf8_lossy(&about.stdout).to_string();

    let npm = std::process::Command::new("pnpm")
        .args(["--dir", "ui", "licenses", "list", "--json"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("pnpm introuvable : {e}"))?;
    if !npm.status.success() {
        return Err(format!(
            "pnpm licenses list a échoué : {}",
            String::from_utf8_lossy(&npm.stderr)
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(&npm.stdout)
        .map_err(|e| format!("JSON pnpm licenses illisible : {e}"))?;
    let groups = json
        .as_object()
        .ok_or("format pnpm licenses inattendu (objet attendu)")?;

    let mut npm_part = String::from(
        "## Composants npm (interface)\n\n\
         Dépendances JavaScript intégrées au bundle (production ; des outils de\n\
         build non distribués peuvent apparaître — sur-inclusion volontaire).\n\
         Généré par `pnpm licenses list` via `cargo run -p xtask -- licenses`.\n\n",
    );
    let mut names: Vec<&String> = groups.keys().collect();
    names.sort();
    for license in names {
        let Some(pkgs) = groups[license].as_array() else {
            continue;
        };
        npm_part.push_str(&format!("### {license}\n\n"));
        let mut listed = Vec::new();
        let mut texts: Vec<(String, String)> = Vec::new();
        for p in pkgs {
            let name = p["name"].as_str().unwrap_or("?");
            // pnpm licenses --json : `versions` et `paths` sont des tableaux.
            let versions: Vec<&str> = p["versions"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let version = if versions.is_empty() {
                "?".to_string()
            } else {
                versions.join(", ")
            };
            listed.push(format!("`{name} {version}`"));
            let paths: Vec<&str> = p["paths"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            for path in paths {
                for cand in [
                    "LICENSE",
                    "LICENSE.md",
                    "LICENSE.txt",
                    "LICENCE",
                    "COPYING",
                    "license.md",
                    "LICENSE-MIT",
                    "LICENSE-APACHE",
                ] {
                    if let Ok(text) = fs::read_to_string(Path::new(path).join(cand)) {
                        if !texts.iter().any(|(t, _)| *t == text) {
                            texts.push((text, format!("{name} {version}")));
                        }
                        break;
                    }
                }
            }
        }
        listed.sort();
        listed.dedup();
        npm_part.push_str(&format!("Paquets : {}\n\n", listed.join(", ")));
        for (text, src) in texts {
            npm_part.push_str(&format!("Texte ({src}) :\n\n```text\n{text}\n```\n\n"));
        }
    }

    let doc = format!(
        "# Licences tierces — HarakaTS\n\n\
         HarakaTS est distribué sous licence **MIT** (voir `LICENSE`). Ce fichier\n\
         rassemble les avis et textes de licence des composants tiers intégrés,\n\
         conformément aux obligations Apache-2.0 / BSD / ISC.\n\n\
         **Généré** par `cargo run -p xtask -- licenses` (cargo about + pnpm\n\
         licenses) — ne pas éditer à la main. Les bibliothèques système liées\n\
         dynamiquement (GTK3, glib, VTE, WebKitGTK — LGPL-2.1+) proviennent des\n\
         paquets de la distribution et ne sont pas redistribuées ici.\n\n\
         {rust_part}\n{npm_part}"
    );
    fs::write(root.join("THIRD-PARTY-LICENSES.md"), &doc)
        .map_err(|e| format!("écriture THIRD-PARTY-LICENSES.md : {e}"))?;
    println!("THIRD-PARTY-LICENSES.md généré ({} octets)", doc.len());
    Ok(())
}

#[cfg(test)]
mod version_tests {
    use super::{is_semver, replace_first_version};

    #[test]
    fn json_key_is_preserved_and_value_replaced() {
        let src = "{\n  \"name\": \"x\",\n  \"version\": \"0.7.0\",\n  \"v\": \"1\"\n}\n";
        let (out, old) = replace_first_version(src, "\"version\":", "0.8.0").unwrap();
        assert_eq!(old, "0.7.0");
        assert!(out.contains("  \"version\": \"0.8.0\","));
        assert!(out.contains("\"v\": \"1\""));
    }

    #[test]
    fn toml_version_line_only() {
        let src = "[workspace.package]\nversion = \"0.7.0\"\nserde = { version = \"1\" }\n";
        let (out, old) = replace_first_version(src, "version = ", "0.8.0").unwrap();
        assert_eq!(old, "0.7.0");
        assert!(out.contains("version = \"0.8.0\"\nserde = { version = \"1\" }"));
    }

    #[test]
    fn semver_validation() {
        assert!(is_semver("0.8.0") && is_semver("1.0.0-rc.1") && is_semver("1.2.3+abc"));
        assert!(!is_semver("1.0") && !is_semver("abc") && !is_semver("1.x.0"));
    }
}
