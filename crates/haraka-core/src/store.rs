//! Répertoires de données, écritures atomiques, sauvegardes rotatives,
//! permissions 0700/0600 (ARCHITECTURE.md §14, SECURITY.md §6).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

pub const BACKUP_COUNT: u32 = 5;

#[derive(Clone, Debug)]
pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    /// Ouvre (ou crée) le répertoire de données avec des permissions privées.
    pub fn open(root: PathBuf) -> Result<Self> {
        create_private_dirs(&root)?;
        set_private_dir(&root)?;
        for sub in ["backups", "logs", "tmp"] {
            let p = root.join(sub);
            create_private_dirs(&p)?;
            set_private_dir(&p)?;
        }
        let dd = Self { root };
        dd.purge_tmp()?;
        Ok(dd)
    }

    /// Emplacement par défaut : `$XDG_DATA_HOME/harakats` ou
    /// `~/.local/share/harakats`.
    pub fn default_path() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
            && !xdg.is_empty()
        {
            return PathBuf::from(xdg).join("harakats");
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("harakats")
    }

    /// Mode portable : marqueur `portable.marker` à côté du binaire → `./data`.
    pub fn portable_path() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?;
        if dir.join("portable.marker").exists() {
            Some(dir.join("data"))
        } else {
            None
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn vault_file(&self) -> PathBuf {
        self.root.join("vault.hrk")
    }

    pub fn backup_file(&self, n: u32) -> PathBuf {
        self.root.join("backups").join(format!("vault-{n}.hrk"))
    }

    pub fn audit_file(&self) -> PathBuf {
        self.root.join("audit.log")
    }

    pub fn known_hosts_file(&self) -> PathBuf {
        self.root.join("known_hosts")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// Racine des temporaires d'édition distante : `$XDG_RUNTIME_DIR/harakats-edit-<id>`
    /// (tmpfs, 0700, jamais sur le disque) si disponible, sinon `<données>/tmp`.
    pub fn edit_tmp_dir(&self) -> PathBuf {
        use sha2::{Digest, Sha256};
        let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) else {
            return self.tmp_dir();
        };
        if !rt.is_dir() {
            return self.tmp_dir();
        }
        let id: String = Sha256::digest(self.root.to_string_lossy().as_bytes())[..4]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let dir = rt.join(format!("harakats-edit-{id}"));
        if fs::create_dir_all(&dir).is_ok() && set_private_dir(&dir).is_ok() {
            dir
        } else {
            self.tmp_dir()
        }
    }

    pub fn vault_exists(&self) -> bool {
        self.vault_file().exists()
    }

    /// Purge des temporaires d'édition distante (y compris après un crash).
    pub fn purge_tmp(&self) -> Result<()> {
        for tmp in [self.tmp_dir(), self.edit_tmp_dir()] {
            if tmp.exists() {
                for entry in fs::read_dir(&tmp)? {
                    let path = entry?.path();
                    secure_remove_all(&path)?;
                }
            }
        }
        Ok(())
    }
}

/// Supprime un fichier ou un dossier en écrasant d'abord le contenu des fichiers (zéros + fsync).
/// Au mieux (SSD/journalisation : pas de garantie forte) : réduit surtout les restes lisibles.
pub fn secure_remove_all(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            secure_remove_all(&entry?.path())?;
        }
        fs::remove_dir(path)?;
    } else {
        if meta.is_file() {
            overwrite_zero(path, meta.len());
        }
        fs::remove_file(path)?;
    }
    Ok(())
}

fn overwrite_zero(path: &Path, len: u64) {
    let Ok(mut f) = fs::OpenOptions::new().write(true).open(path) else {
        return;
    };
    let zeros = [0u8; 8192];
    let mut left = len;
    while left > 0 {
        let n = left.min(zeros.len() as u64) as usize;
        if f.write_all(&zeros[..n]).is_err() {
            return;
        }
        left -= n as u64;
    }
    let _ = f.sync_all();
}

/// Crée un répertoire (et ses parents) directement en 0700 : pas de fenêtre en permissions par défaut.
fn create_private_dirs(path: &Path) -> Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)?;
    Ok(())
}

/// Écriture atomique : fichier temporaire 0600 (nom aléatoire, création exclusive : un lien
/// symbolique ou un fichier préexistant fait échouer l'ouverture au lieu d'être suivi) → fsync → rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = tmp_sibling(path);
    let r = write_tmp_and_rename(&tmp, path, bytes);
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

fn write_tmp_and_rename(tmp: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(tmp, path)?;
    set_private_file(path)?;
    sync_parent_dir(path);
    Ok(())
}

/// fsync du répertoire parent : le rename survit à une coupure de courant.
#[cfg(unix)]
fn sync_parent_dir(path: &Path) {
    if let Some(dir) = path.parent().and_then(|p| fs::File::open(p).ok()) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) {}

/// Un fichier de coffre est structurellement valide (enveloppe décodable, sans clé).
pub fn envelope_valid(path: &Path) -> bool {
    fs::read(path).is_ok_and(|b| crate::format::Envelope::decode(&b).is_ok())
}

fn tmp_sibling(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
    path.with_file_name(name)
}

/// Rotation : coffre courant → backup-1, décalage 1→2 … 4→5, le 5e est remplacé.
pub fn rotate_backups(dd: &DataDir) -> Result<()> {
    let vault = dd.vault_file();
    if !vault.exists() {
        return Ok(());
    }
    // Un coffre déjà corrompu ne doit jamais chasser une bonne sauvegarde de la rotation.
    if !envelope_valid(&vault) {
        return Ok(());
    }
    let oldest = dd.backup_file(BACKUP_COUNT);
    if oldest.exists() {
        fs::remove_file(&oldest)?;
    }
    for n in (1..BACKUP_COUNT).rev() {
        let src = dd.backup_file(n);
        if src.exists() {
            fs::rename(&src, dd.backup_file(n + 1))?;
        }
    }
    fs::copy(&vault, dd.backup_file(1))?;
    set_private_file(&dd.backup_file(1))?;
    Ok(())
}

/// (index, date de modification, structure valide).
pub fn list_backups(dd: &DataDir) -> Vec<(u32, Option<std::time::SystemTime>, bool)> {
    (1..=BACKUP_COUNT)
        .filter(|n| dd.backup_file(*n).exists())
        .map(|n| {
            let modified = fs::metadata(dd.backup_file(n))
                .and_then(|m| m.modified())
                .ok();
            (n, modified, envelope_valid(&dd.backup_file(n)))
        })
        .collect()
}

/// Restauration d'une sauvegarde (écriture atomique par-dessus le coffre).
pub fn restore_backup(dd: &DataDir, n: u32) -> Result<()> {
    let backup = dd.backup_file(n);
    if !backup.exists() {
        return Err(Error::NotFound("sauvegarde"));
    }
    let bytes = fs::read(&backup)?;
    if crate::format::Envelope::decode(&bytes).is_err() {
        return Err(Error::BadFormat("sauvegarde corrompue"));
    }
    atomic_write(&dd.vault_file(), &bytes)
}

pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    Ok(fs::read(path)?)
}

#[cfg(unix)]
pub fn set_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
pub fn set_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
pub fn set_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
pub fn set_private_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
pub fn check_permissions(dd: &DataDir) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let mut warnings = Vec::new();
    let root_mode = fs::metadata(dd.root())
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0);
    if root_mode != 0o700 {
        warnings.push(format!(
            "répertoire de données en {root_mode:o} (attendu 700)"
        ));
    }
    let mode_of = |p: &Path| {
        fs::metadata(p)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0)
    };
    for dir in [dd.root().join("backups"), dd.logs_dir()] {
        if dir.exists() && mode_of(&dir) != 0o700 {
            warnings.push(format!(
                "{} en {:o} (attendu 700)",
                dir.display(),
                mode_of(&dir)
            ));
        }
    }
    let mut files = vec![dd.vault_file(), dd.audit_file(), dd.known_hosts_file()];
    if let Ok(rd) = fs::read_dir(dd.root().join("backups")) {
        files.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_file()));
    }
    for f in files {
        if f.exists() && mode_of(&f) != 0o600 {
            warnings.push(format!(
                "{} en {:o} (attendu 600)",
                f.display(),
                mode_of(&f)
            ));
        }
    }
    warnings
}

#[cfg(not(unix))]
pub fn check_permissions(_dd: &DataDir) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_vault_never_displaces_good_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let dd = DataDir::open(tmp.path().to_path_buf()).unwrap();
        // Faux coffre non décodable + une « bonne » sauvegarde existante.
        fs::write(dd.vault_file(), b"garbage").unwrap();
        fs::write(dd.backup_file(1), b"previous-good").unwrap();
        rotate_backups(&dd).unwrap();
        assert_eq!(fs::read(dd.backup_file(1)).unwrap(), b"previous-good");
        assert!(!dd.backup_file(2).exists());
        // Une sauvegarde invalide ne peut pas être restaurée.
        assert!(restore_backup(&dd, 1).is_err());
        assert!(list_backups(&dd).iter().all(|(_, _, valid)| !valid));
    }

    #[test]
    fn secure_remove_deletes_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("doc");
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("a.txt"), b"secret").unwrap();
        fs::write(d.join("sub/b.txt"), b"secret").unwrap();
        secure_remove_all(&d).unwrap();
        assert!(!d.exists());
    }

    #[test]
    fn atomic_write_leaves_no_tmp_and_uses_unique_names() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("vault.hrk");
        atomic_write(&f, b"un").unwrap();
        atomic_write(&f, b"deux").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"deux");
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 1, "aucun .tmp résiduel : {names:?}");
        assert_ne!(tmp_sibling(&f), tmp_sibling(&f));
    }

    #[cfg(unix)]
    #[test]
    fn new_data_dirs_are_private_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("data");
        let dd = DataDir::open(root.clone()).unwrap();
        for p in [
            root.clone(),
            root.join("backups"),
            root.join("logs"),
            root.join("tmp"),
        ] {
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert!(check_permissions(&dd).is_empty());
    }
}
