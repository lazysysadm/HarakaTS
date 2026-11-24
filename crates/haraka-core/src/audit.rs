//! Journal d'audit local, sans aucun secret (SECURITY.md §9).
//! JSON lines, 0600, rotation à 1 MiB (3 fichiers conservés).

use std::fs;
use std::io::Write;

use crate::crypto::now_rfc3339;
use crate::error::Result;
use crate::store::{DataDir, set_private_file};

const MAX_AUDIT_LEN: u64 = 1 << 20;
const AUDIT_ROTATIONS: u32 = 3;

pub fn append(dd: &DataDir, event: &str, target: Option<&str>, detail: Option<&str>) -> Result<()> {
    let path = dd.audit_file();
    rotate_if_needed(&path)?;
    let line = serde_json::json!({
        "ts": now_rfc3339(),
        "event": event,
        "target": target,
        "detail": detail,
    });
    let mut opts = fs::OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&path)?;
    writeln!(file, "{line}")?;
    set_private_file(&path)?;
    Ok(())
}

fn rotate_if_needed(path: &std::path::Path) -> Result<()> {
    let len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len < MAX_AUDIT_LEN {
        return Ok(());
    }
    let rotated = |n: u32| path.with_file_name(format!("audit.log.{n}"));
    let oldest = rotated(AUDIT_ROTATIONS);
    if oldest.exists() {
        fs::remove_file(&oldest)?;
    }
    for n in (1..AUDIT_ROTATIONS).rev() {
        let src = rotated(n);
        if src.exists() {
            fs::rename(&src, rotated(n + 1))?;
        }
    }
    fs::rename(path, rotated(1))?;
    Ok(())
}

/// Dernières entrées (ordre chronologique), tous fichiers de rotation confondus.
pub fn list(dd: &DataDir, limit: usize) -> Result<Vec<serde_json::Value>> {
    let mut entries = Vec::new();
    let mut files = vec![dd.audit_file()];
    for n in 1..=AUDIT_ROTATIONS {
        files.push(dd.audit_file().with_file_name(format!("audit.log.{n}")));
    }
    // Du plus ancien au plus récent.
    for path in files.iter().rev() {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(path)?;
        for line in content.lines() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                entries.push(value);
            }
        }
    }
    let skip = entries.len().saturating_sub(limit);
    Ok(entries.into_iter().skip(skip).collect())
}
