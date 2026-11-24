//! Opérations SFTP de base (jalon M5) : listing, mkdir, rename, suppression,
//! chmod, propriétés, liens. Fonctions libres sur `SftpSession` : la session
//! est obtenue via `SessionManager::sftp` (même connexion SSH que le terminal).
//!
//! Chemins distants : toujours POSIX (`/`), jamais interprétés par le shell.

use std::future::Future;
use std::pin::Pin;

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, FileType};
use serde::Serialize;

use crate::error::{Result, SshError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    Other,
}

impl From<FileType> for EntryKind {
    fn from(t: FileType) -> Self {
        match t {
            FileType::Dir => Self::Dir,
            FileType::File => Self::File,
            FileType::Symlink => Self::Symlink,
            FileType::Other => Self::Other,
        }
    }
}

/// Entrée de répertoire distant (sérialisable vers l'UI).
#[derive(Debug, Clone, Serialize)]
pub struct RemoteEntry {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    /// Secondes epoch (UTC).
    pub mtime: Option<u64>,
    /// Bits de permission (0o7777), sans les bits de type.
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub user: Option<String>,
    pub group: Option<String>,
    /// Pour un lien symbolique : cible lue (`readlink`).
    pub link_target: Option<String>,
    /// Pour un lien symbolique : la cible est-elle un répertoire ?
    pub link_to_dir: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    /// Chemin canonique du répertoire listé.
    pub path: String,
    pub entries: Vec<RemoteEntry>,
}

fn sftp_err(e: russh_sftp::client::error::Error) -> SshError {
    SshError::Sftp(e.to_string())
}

/// Erreur de canal/protocole (≠ refus applicatif « permission denied », « no
/// such file ») : la session SFTP est probablement morte → à invalider.
pub fn is_fatal(e: &SshError) -> bool {
    match e {
        SshError::Sftp(msg) => {
            let m = msg.to_lowercase();
            !(m.contains("permission denied")
                || m.contains("no such file")
                || m.contains("failure")
                || m.contains("already exists")
                || m.contains("not a directory")
                || m.contains("directory not empty"))
                && (m.contains("closed")
                    || m.contains("timeout")
                    || m.contains("channel")
                    || m.contains("unexpected")
                    || m.contains("io error"))
        }
        SshError::Closed | SshError::Russh(_) => true,
        _ => false,
    }
}

/// Joint un nom à un répertoire (POSIX, sans doublon de `/`).
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Nom de base d'un chemin POSIX.
pub fn base_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

fn entry_from(dir: &str, name: String, attrs: &FileAttributes) -> RemoteEntry {
    RemoteEntry {
        path: join(dir, &name),
        name,
        kind: attrs.file_type().into(),
        size: attrs.len(),
        mtime: attrs.mtime.map(u64::from),
        mode: attrs.permissions.map(|p| p & 0o7777),
        uid: attrs.uid,
        gid: attrs.gid,
        user: attrs.user.clone(),
        group: attrs.group.clone(),
        link_target: None,
        link_to_dir: false,
    }
}

/// Chemin canonique (`.` → répertoire de départ, `..` résolus).
pub async fn canonicalize(sftp: &SftpSession, path: &str) -> Result<String> {
    sftp.canonicalize(path).await.map_err(sftp_err)
}

/// Liste un répertoire (sans `.`/`..`). Les liens symboliques sont résolus
/// (`readlink` + `stat`) pour savoir s'ils pointent un répertoire.
pub async fn list(sftp: &SftpSession, path: &str) -> Result<Listing> {
    let canon = canonicalize(sftp, path).await?;
    let read = sftp.read_dir(canon.as_str()).await.map_err(sftp_err)?;
    let mut entries = Vec::new();
    for item in read {
        let attrs = item.metadata();
        let mut entry = entry_from(&canon, item.file_name(), &attrs);
        if entry.kind == EntryKind::Symlink {
            entry.link_target = sftp.read_link(entry.path.as_str()).await.ok();
            if let Ok(target) = sftp.metadata(entry.path.as_str()).await {
                entry.link_to_dir = target.file_type().is_dir();
            }
        }
        entries.push(entry);
    }
    Ok(Listing {
        path: canon,
        entries,
    })
}

/// Propriétés d'un chemin (sans suivre un éventuel lien final).
pub async fn stat(sftp: &SftpSession, path: &str) -> Result<RemoteEntry> {
    let attrs = sftp.symlink_metadata(path).await.map_err(sftp_err)?;
    let dir = path.trim_end_matches('/');
    let parent = dir.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let parent = if parent.is_empty() { "/" } else { parent };
    let mut entry = entry_from(parent, base_name(path).to_string(), &attrs);
    entry.path = path.to_string();
    if entry.kind == EntryKind::Symlink {
        entry.link_target = sftp.read_link(path).await.ok();
        if let Ok(target) = sftp.metadata(path).await {
            entry.link_to_dir = target.file_type().is_dir();
        }
    }
    Ok(entry)
}

pub async fn mkdir(sftp: &SftpSession, path: &str) -> Result<()> {
    sftp.create_dir(path).await.map_err(sftp_err)
}

pub async fn rename(sftp: &SftpSession, from: &str, to: &str) -> Result<()> {
    sftp.rename(from, to).await.map_err(sftp_err)
}

pub async fn symlink(sftp: &SftpSession, link_path: &str, target: &str) -> Result<()> {
    // OpenSSH (de facto standard, contrairement au brouillon SFTP v3) lit les
    // champs dans l'ordre (cible, chemin du lien) : constaté sur sftp-server.
    sftp.symlink(target, link_path).await.map_err(sftp_err)
}

/// Change les permissions (0o7777). Récursif : fichiers et dossiers du sous-arbre
/// (liens ignorés — `setstat` suivrait la cible, hors de l'arbre).
pub async fn chmod(sftp: &SftpSession, path: &str, mode: u32, recursive: bool) -> Result<()> {
    let mode = mode & 0o7777;
    let attrs = FileAttributes {
        permissions: Some(mode),
        ..FileAttributes::empty()
    };
    sftp.set_metadata(path, attrs).await.map_err(sftp_err)?;
    if recursive {
        let meta = sftp.symlink_metadata(path).await.map_err(sftp_err)?;
        if meta.file_type().is_dir() {
            chmod_children(sftp, path, mode).await?;
        }
    }
    Ok(())
}

fn chmod_children<'a>(
    sftp: &'a SftpSession,
    dir: &'a str,
    mode: u32,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let read = sftp.read_dir(dir).await.map_err(sftp_err)?;
        for item in read {
            let child = join(dir, &item.file_name());
            match item.file_type() {
                FileType::Symlink => continue,
                FileType::Dir => {
                    let attrs = FileAttributes {
                        permissions: Some(mode),
                        ..FileAttributes::empty()
                    };
                    sftp.set_metadata(child.as_str(), attrs)
                        .await
                        .map_err(sftp_err)?;
                    chmod_children(sftp, &child, mode).await?;
                }
                _ => {
                    let attrs = FileAttributes {
                        permissions: Some(mode),
                        ..FileAttributes::empty()
                    };
                    sftp.set_metadata(child.as_str(), attrs)
                        .await
                        .map_err(sftp_err)?;
                }
            }
        }
        Ok(())
    })
}

/// Supprime un fichier, un lien ou un dossier. Un dossier non vide exige
/// `recursive` (parcours manuel : SFTP n'a pas de `rm -r`). Les liens sont
/// supprimés eux-mêmes, jamais suivis.
pub async fn remove(sftp: &SftpSession, path: &str, recursive: bool) -> Result<()> {
    let meta = sftp.symlink_metadata(path).await.map_err(sftp_err)?;
    if meta.file_type().is_dir() {
        if recursive {
            remove_children(sftp, path).await?;
        }
        sftp.remove_dir(path).await.map_err(sftp_err)
    } else {
        sftp.remove_file(path).await.map_err(sftp_err)
    }
}

fn remove_children<'a>(
    sftp: &'a SftpSession,
    dir: &'a str,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let read = sftp.read_dir(dir).await.map_err(sftp_err)?;
        for item in read {
            let child = join(dir, &item.file_name());
            if item.file_type().is_dir() {
                remove_children(sftp, &child).await?;
                sftp.remove_dir(child.as_str()).await.map_err(sftp_err)?;
            } else {
                sftp.remove_file(child.as_str()).await.map_err(sftp_err)?;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_and_base_name() {
        assert_eq!(join("/a", "b"), "/a/b");
        assert_eq!(join("/", "b"), "/b");
        assert_eq!(base_name("/a/b/"), "b");
        assert_eq!(base_name("/"), "");
    }

    #[test]
    fn fatal_classification() {
        assert!(!is_fatal(&SshError::Sftp("Permission denied".into())));
        assert!(!is_fatal(&SshError::Sftp("No such file".into())));
        assert!(is_fatal(&SshError::Sftp("channel closed".into())));
        assert!(is_fatal(&SshError::Closed));
    }
}
