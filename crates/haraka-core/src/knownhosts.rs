//! known_hosts interne au format OpenSSH (jalon M3), entrées hashées `|1|`
//! supportées (HMAC-SHA1, comme OpenSSH). Headless : ne dépend pas de russh —
//! les clés sont manipulées sous forme (type, base64 du blob SSH).

use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD as B64, STANDARD_NO_PAD as B64NP};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::store::atomic_write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnownHostStatus {
    Match,
    Unknown,
    Changed {
        old_fingerprint: String,
    },
    /// Clé listée `@revoked` : connexion à refuser, sans prompt (comme OpenSSH).
    Revoked,
}

#[derive(Clone, Debug)]
pub struct KnownHostEntry {
    pub host_field: String,
    pub key_type: String,
    pub key_b64: String,
    pub fingerprint: String,
}

/// Marqueur OpenSSH : `host` si port 22, sinon `[host]:port`.
pub fn host_marker(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// Empreinte SHA256 format OpenSSH : `SHA256:<base64 sans padding>`.
pub fn fingerprint(key_b64: &str) -> Result<String> {
    let blob = B64
        .decode(key_b64)
        .map_err(|_| Error::BadFormat("clé publique base64 invalide"))?;
    let digest = Sha256::digest(&blob);
    Ok(format!("SHA256:{}", B64NP.encode(digest)))
}

fn hashed_matches(field: &str, marker: &str) -> bool {
    // Format : |1|salt_b64|hash_b64
    let parts: Vec<&str> = field.split('|').collect();
    if parts.len() != 4 || !parts[0].is_empty() || parts[1] != "1" {
        return false;
    }
    let (Ok(salt), Ok(expected)) = (B64.decode(parts[2]), B64.decode(parts[3])) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(&salt) else {
        return false;
    };
    mac.update(marker.as_bytes());
    mac.finalize().into_bytes().as_slice() == expected.as_slice()
}

fn line_matches_host(host_field: &str, marker: &str) -> bool {
    host_field
        .split(',')
        .any(|m| m == marker || hashed_matches(m, marker))
}

/// Vérifie une clé d'hôte contre le fichier interne.
pub fn check(
    path: &Path,
    host: &str,
    port: u16,
    key_type: &str,
    key_b64: &str,
) -> Result<KnownHostStatus> {
    if !path.exists() {
        return Ok(KnownHostStatus::Unknown);
    }
    let marker = host_marker(host, port);
    let content = fs::read_to_string(path)?;
    let mut changed: Option<String> = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let first = fields.next();
        // `@revoked` : la clé est refusée si elle correspond (hôte `*` ou hôte courant).
        // `@cert-authority` et autres marqueurs : ignorés (non gérés).
        if first == Some("@revoked") {
            if let (Some(hf), Some(_), Some(kb)) = (fields.next(), fields.next(), fields.next())
                && kb == key_b64
                && (hf == "*" || line_matches_host(hf, &marker))
            {
                return Ok(KnownHostStatus::Revoked);
            }
            continue;
        }
        if line.starts_with('@') {
            continue;
        }
        let (Some(host_field), Some(kt), Some(kb)) = (first, fields.next(), fields.next()) else {
            continue;
        };
        if !line_matches_host(host_field, &marker) {
            continue;
        }
        if kt == key_type {
            if kb == key_b64 {
                return Ok(KnownHostStatus::Match);
            }
            // Ligne corrompue (base64 invalide) pour cet hôte : on ne retombe JAMAIS sur « inconnu »
            // (TOFU), la clé est considérée comme changée.
            changed = Some(fingerprint(kb).unwrap_or_else(|_| "(entrée illisible)".to_string()));
        }
    }
    match changed {
        Some(old_fingerprint) => Ok(KnownHostStatus::Changed { old_fingerprint }),
        None => Ok(KnownHostStatus::Unknown),
    }
}

/// Mémorise une clé (TOFU ou remplacement explicite après changement).
/// Les anciennes entrées du même hôte + type sont retirées.
pub fn learn(path: &Path, host: &str, port: u16, key_type: &str, key_b64: &str) -> Result<()> {
    let marker = host_marker(host, port);
    let mut lines: Vec<String> = if path.exists() {
        fs::read_to_string(path)?
            .lines()
            .filter(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with('@') {
                    return true;
                }
                let mut fields = line.split_whitespace();
                let (Some(hf), Some(kt)) = (fields.next(), fields.next()) else {
                    return true;
                };
                !(line_matches_host(hf, &marker) && kt == key_type)
            })
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    lines.push(format!("{marker} {key_type} {key_b64}"));
    let mut out = lines.join("\n");
    out.push('\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(path, out.as_bytes())
}

/// Oublie explicitement les entrées d'un hôte (remplacement volontaire d'une clé changée).
/// Retourne le nombre de lignes retirées.
pub fn forget(path: &Path, host_field: &str, fingerprint_wanted: &str) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    let kept: Vec<String> = fs::read_to_string(path)?
        .lines()
        .filter(|line| {
            let mut f = line.split_whitespace();
            let hit = matches!(
                (f.next(), f.next(), f.next()),
                (Some(h), Some(_), Some(k))
                    if h == host_field && fingerprint(k).is_ok_and(|fp| fp == fingerprint_wanted)
            );
            if hit {
                removed += 1;
            }
            !hit
        })
        .map(str::to_string)
        .collect();
    if removed > 0 {
        let mut out = kept.join("\n");
        out.push('\n');
        atomic_write(path, out.as_bytes())?;
    }
    Ok(removed)
}

/// Vide le known_hosts INTERNE de l'application (jamais `~/.ssh/known_hosts`). Une copie de
/// sécurité est écrite avant dans `known_hosts.BAK` (écrasée). Retourne le nombre d'entrées retirées.
pub fn clear(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let count = fs::read_to_string(path)?
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .count();
    if count == 0 {
        return Ok(0);
    }
    fs::copy(path, path.with_file_name("known_hosts.BAK"))?;
    atomic_write(path, b"")?;
    Ok(count)
}

/// Import du known_hosts système : lignes valides copiées, doublons ignorés.
/// Les entrées hashées sont conservées telles quelles (vérifiées au check).
pub fn import_system(src: &Path, dst: &Path) -> Result<usize> {
    if !src.exists() {
        return Err(Error::NotFound("known_hosts source"));
    }
    let existing: Vec<String> = if dst.exists() {
        fs::read_to_string(dst)?
            .lines()
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    let mut added = 0;
    let mut out = existing.clone();
    for line in fs::read_to_string(src)?.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Les clés `@revoked` sont importées (honorées par `check`) ; les autres marqueurs sont ignorés.
        let revoked = line.starts_with("@revoked ");
        if line.starts_with('@') && !revoked {
            continue;
        }
        if line.split_whitespace().count() < if revoked { 4 } else { 3 } {
            continue;
        }
        if !existing.iter().any(|l| l == line) && !out.iter().any(|l| l == line) {
            out.push(line.to_string());
            added += 1;
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(dst, text.as_bytes())?;
    Ok(added)
}

/// Liste les entrées (pour l'UI).
pub fn list(path: &Path) -> Result<Vec<KnownHostEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for line in fs::read_to_string(path)?.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('@') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(host_field), Some(key_type), Some(key_b64)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        out.push(KnownHostEntry {
            host_field: host_field.to_string(),
            key_type: key_type.to_string(),
            key_b64: key_b64.to_string(),
            fingerprint: fingerprint(key_b64)?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blob SSH ed25519 valide (type + clé de 32 octets), encodé base64.
    fn key_b64(seed: u8) -> String {
        let mut blob = Vec::new();
        blob.extend_from_slice(&11u32.to_be_bytes());
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&32u32.to_be_bytes());
        blob.extend(std::iter::repeat_n(seed, 32));
        B64.encode(&blob)
    }

    fn tmp() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        (dir, path)
    }

    #[test]
    fn clear_empties_file_and_keeps_backup() {
        let (d, path) = tmp();
        assert_eq!(clear(&path).unwrap(), 0);
        for seed in [1u8, 2] {
            check(
                &path,
                &format!("h{seed}"),
                22,
                "ssh-ed25519",
                &key_b64(seed),
            )
            .unwrap();
            learn(
                &path,
                &format!("h{seed}"),
                22,
                "ssh-ed25519",
                &key_b64(seed),
            )
            .unwrap();
        }
        assert_eq!(clear(&path).unwrap(), 2);
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
        let bak = fs::read_to_string(d.path().join("known_hosts.BAK")).unwrap();
        assert_eq!(bak.lines().count(), 2);
        // Après effacement, l'hôte redevient inconnu (nouveau TOFU).
        assert_eq!(
            check(&path, "h1", 22, "ssh-ed25519", &key_b64(1)).unwrap(),
            KnownHostStatus::Unknown
        );
    }

    #[test]
    fn tofu_learn_match_changed() {
        let (_d, path) = tmp();
        assert_eq!(
            check(&path, "h", 22, "ssh-ed25519", &key_b64(1)).unwrap(),
            KnownHostStatus::Unknown
        );
        learn(&path, "h", 22, "ssh-ed25519", &key_b64(1)).unwrap();
        assert_eq!(
            check(&path, "h", 22, "ssh-ed25519", &key_b64(1)).unwrap(),
            KnownHostStatus::Match
        );
        let changed = check(&path, "h", 22, "ssh-ed25519", &key_b64(2)).unwrap();
        assert!(matches!(changed, KnownHostStatus::Changed { .. }));
        // Remplacement explicite
        learn(&path, "h", 22, "ssh-ed25519", &key_b64(2)).unwrap();
        assert_eq!(
            check(&path, "h", 22, "ssh-ed25519", &key_b64(2)).unwrap(),
            KnownHostStatus::Match
        );
        assert_eq!(list(&path).unwrap().len(), 1);
    }

    #[test]
    fn non_standard_port_marker() {
        let (_d, path) = tmp();
        learn(&path, "h", 2222, "ssh-ed25519", &key_b64(1)).unwrap();
        assert_eq!(
            check(&path, "h", 2222, "ssh-ed25519", &key_b64(1)).unwrap(),
            KnownHostStatus::Match
        );
        assert_eq!(
            check(&path, "h", 22, "ssh-ed25519", &key_b64(1)).unwrap(),
            KnownHostStatus::Unknown
        );
        assert!(list(&path).unwrap()[0].host_field == "[h]:2222");
    }

    #[test]
    fn hashed_entry_matches() {
        let (_d, path) = tmp();
        let salt = b"0123456789abcdef";
        let mut mac = Hmac::<Sha1>::new_from_slice(salt).unwrap();
        mac.update(b"h");
        let hash = mac.finalize().into_bytes();
        let line = format!(
            "|1|{}|{} ssh-ed25519 {}",
            B64.encode(salt),
            B64.encode(hash),
            key_b64(1)
        );
        fs::write(&path, format!("{line}\n")).unwrap();
        assert_eq!(
            check(&path, "h", 22, "ssh-ed25519", &key_b64(1)).unwrap(),
            KnownHostStatus::Match
        );
        assert_eq!(
            check(&path, "autre", 22, "ssh-ed25519", &key_b64(1)).unwrap(),
            KnownHostStatus::Unknown
        );
    }

    #[test]
    fn import_dedupes_and_keeps_hashed() {
        let (_d, path) = tmp();
        let src = path.parent().unwrap().join("system_known_hosts");
        let key_a = &key_b64(1);
        let key_b = &key_b64(2);
        fs::write(
            &src,
            format!(
                "# commentaire\nh ssh-ed25519 {key_a}\n|h|1|xx ssh-ed25519 {key_b}\n\ninvalide\n"
            ),
        )
        .unwrap();
        let n = import_system(&src, &path).unwrap();
        assert_eq!(n, 2);
        let n2 = import_system(&src, &path).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn fingerprint_format() {
        let fp = fingerprint(&key_b64(1)).unwrap();
        assert!(fp.starts_with("SHA256:"));
        assert!(!fp.contains('='));
    }

    #[test]
    fn revoked_key_is_refused() {
        let (_d, path) = tmp();
        let k = key_b64(9);
        std::fs::write(&path, format!("@revoked * ssh-ed25519 {k}\n")).unwrap();
        assert_eq!(
            check(&path, "h.example", 22, "ssh-ed25519", &k).unwrap(),
            KnownHostStatus::Revoked
        );
        // une autre clé n'est pas touchée
        assert_eq!(
            check(&path, "h.example", 22, "ssh-ed25519", &key_b64(8)).unwrap(),
            KnownHostStatus::Unknown
        );
    }

    #[test]
    fn corrupt_line_for_host_is_changed_not_unknown() {
        let (_d, path) = tmp();
        std::fs::write(&path, "h.example ssh-ed25519 !!!pas-du-base64!!!\n").unwrap();
        assert!(matches!(
            check(&path, "h.example", 22, "ssh-ed25519", &key_b64(1)).unwrap(),
            KnownHostStatus::Changed { .. }
        ));
    }

    #[test]
    fn unreadable_file_is_an_error() {
        let (_d, path) = tmp();
        std::fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();
        assert!(check(&path, "h.example", 22, "ssh-ed25519", &key_b64(1)).is_err());
    }
}
