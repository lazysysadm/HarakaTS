//! Journalisation de session (M7), opt-in par connexion.
//!
//! - `AnsiStripper` : transforme le flux terminal (séquences CSI/OSC/DCS,
//!   sélections de jeu de caractères, ESC isolés, `\r`, `\b`) en lignes de
//!   texte brut. Streaming : une séquence coupée entre deux blocs est reprise.
//! - `SessionLog` : fichier `<logs>/<connexion>-<AAAAMMJJ-HHMMSS>.log` en 0600
//!   (dossier 0700), en-tête sans secret, vidage périodique, plafond de taille.
//!
//! Avertissement : le journal contient ce que le terminal affiche ; un secret
//! saisi à un prompt qui l'écho (ou collé en clair) s'y retrouve. D'où l'opt-in.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::crypto::{self, DEK_LEN, NONCE_LEN, now_rfc3339, random_array};
use crate::error::{Error, Result};
use crate::store::{DataDir, set_private_dir, set_private_file};

/// Taille maximale d'un fichier de journal (50 MiB).
pub const DEFAULT_CAP: u64 = 50 * 1024 * 1024;
/// Intervalle de vidage du tampon disque.
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(2);
/// Chaîne de contrôle (OSC/DCS…) non terminée : abandon au-delà de cette taille.
const MAX_STRING_LEN: usize = 8192;
const CAP_NOTE: &str = "[HarakaTS] size limit reached - logging stopped";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    Ground,
    Esc,
    /// Séquence `ESC <intermédiaires 0x20-0x2F> <final>` (ex. `ESC ( B`).
    EscInter,
    Csi,
    Osc,
    OscEsc,
    /// DCS / SOS / PM / APC : terminées par ST (`ESC \`) ou BEL.
    Str,
    StrEsc,
}

/// Retire les séquences d'échappement et rend des lignes de texte brut.
#[derive(Debug)]
pub struct AnsiStripper {
    st: St,
    line: Vec<char>,
    cursor: usize,
    str_len: usize,
}

impl Default for AnsiStripper {
    fn default() -> Self {
        Self::new()
    }
}

impl AnsiStripper {
    pub fn new() -> Self {
        Self {
            st: St::Ground,
            line: Vec::new(),
            cursor: 0,
            str_len: 0,
        }
    }

    /// Traite un bloc de texte ; les lignes terminées (`\n`) sont ajoutées à `out`.
    pub fn feed(&mut self, text: &str, out: &mut Vec<String>) {
        for c in text.chars() {
            self.step(c, out);
        }
    }

    /// Ligne partielle restante (fin de flux).
    pub fn finish(&mut self) -> Option<String> {
        self.st = St::Ground;
        if self.line.is_empty() {
            return None;
        }
        let s = self.take_line();
        Some(s)
    }

    fn take_line(&mut self) -> String {
        let s: String = self.line.drain(..).collect();
        self.cursor = 0;
        s.trim_end_matches(' ').to_string()
    }

    fn put(&mut self, c: char) {
        if self.cursor < self.line.len() {
            self.line[self.cursor] = c;
        } else {
            self.line.push(c);
        }
        self.cursor += 1;
    }

    fn ground(&mut self, c: char, out: &mut Vec<String>) {
        match c {
            '\x1b' => self.st = St::Esc,
            '\n' => {
                let s = self.take_line();
                out.push(s);
            }
            // `\r\n` → saut de ligne simple ; `\r` seul = retour en colonne 0
            // (les caractères suivants écrasent : barres de progression).
            '\r' => self.cursor = 0,
            '\x08' => self.cursor = self.cursor.saturating_sub(1),
            '\t' => self.put('\t'),
            c if (c as u32) < 0x20 || c == '\x7f' || ('\u{80}'..='\u{9f}').contains(&c) => {}
            c => self.put(c),
        }
    }

    fn step(&mut self, c: char, out: &mut Vec<String>) {
        match self.st {
            St::Ground => self.ground(c, out),
            St::Esc => match c {
                '[' => self.st = St::Csi,
                ']' => {
                    self.st = St::Osc;
                    self.str_len = 0;
                }
                'P' | 'X' | '^' | '_' => {
                    self.st = St::Str;
                    self.str_len = 0;
                }
                '\x1b' => {}
                ' '..='/' => self.st = St::EscInter,
                c if (c as u32) < 0x20 => {
                    self.st = St::Ground;
                    self.ground(c, out);
                }
                // ESC 7, ESC =, ESC c, ESC M… : séquence à un caractère.
                _ => self.st = St::Ground,
            },
            St::EscInter => match c {
                ' '..='/' => {}
                '\x1b' => self.st = St::Esc,
                c if (c as u32) < 0x20 => {
                    self.st = St::Ground;
                    self.ground(c, out);
                }
                _ => self.st = St::Ground,
            },
            St::Csi => match c {
                '\u{40}'..='\u{7e}' => self.st = St::Ground,
                '\x1b' => self.st = St::Esc,
                '\u{20}'..='\u{3f}' => {}
                // Caractère de contrôle dans une CSI : séquence mal formée.
                c => {
                    self.st = St::Ground;
                    self.ground(c, out);
                }
            },
            St::Osc => {
                self.str_len += 1;
                match c {
                    '\x07' | '\u{9c}' => self.st = St::Ground,
                    '\x1b' => self.st = St::OscEsc,
                    _ if self.str_len > MAX_STRING_LEN => self.st = St::Ground,
                    _ => {}
                }
            }
            St::OscEsc | St::StrEsc => match c {
                '\\' => self.st = St::Ground,
                '\x1b' => {}
                // ESC suivi d'autre chose : la chaîne est abandonnée, nouvelle séquence.
                _ => {
                    self.st = St::Esc;
                    self.step(c, out);
                }
            },
            St::Str => {
                self.str_len += 1;
                match c {
                    '\x07' | '\u{9c}' => self.st = St::Ground,
                    '\x1b' => self.st = St::StrEsc,
                    _ if self.str_len > MAX_STRING_LEN => self.st = St::Ground,
                    _ => {}
                }
            }
        }
    }
}

/// Nom de fichier sûr dérivé du nom de connexion.
pub fn sanitize_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() { "session".into() } else { s }
}

/// Magic d'un journal chiffré (`.log.enc`).
pub const ENC_MAGIC: &[u8; 8] = b"HRKLOG01";
/// Au-delà de cette taille de bloc chiffré, le fichier est jugé invalide.
const MAX_RECORD: usize = 4 * 1024 * 1024;
/// Taille du tampon avant écriture forcée.
const BUF_LIMIT: usize = 64 * 1024;

/// Sous-clé de journal dérivée de la DEK (jamais la DEK elle-même).
fn log_key(dek: &[u8; DEK_LEN]) -> Zeroizing<[u8; DEK_LEN]> {
    let mut h = Sha256::new();
    h.update(b"harakats-session-log-v1");
    h.update(dek);
    Zeroizing::new(h.finalize().into())
}

fn record_nonce(prefix: &[u8; 16], counter: u64) -> [u8; NONCE_LEN] {
    let mut n = [0u8; NONCE_LEN];
    n[..16].copy_from_slice(prefix);
    n[16..].copy_from_slice(&counter.to_be_bytes());
    n
}

fn record_aad(counter: u64) -> Vec<u8> {
    let mut a = ENC_MAGIC.to_vec();
    a.extend_from_slice(&counter.to_be_bytes());
    a
}

struct LogCipher {
    key: Zeroizing<[u8; DEK_LEN]>,
    prefix: [u8; 16],
    counter: u64,
}

/// Destination d'écriture : clair, ou blocs XChaCha20-Poly1305 `[u32 len][ct]`.
struct Sink {
    file: File,
    buf: Vec<u8>,
    cipher: Option<LogCipher>,
}

impl Sink {
    fn put(&mut self, line: &str) -> std::io::Result<()> {
        self.buf.extend_from_slice(line.as_bytes());
        self.buf.push(b'\n');
        if self.buf.len() >= BUF_LIMIT {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            match self.cipher.as_mut() {
                None => self.file.write_all(&self.buf)?,
                Some(c) => {
                    let nonce = record_nonce(&c.prefix, c.counter);
                    let ct = crypto::seal(&c.key, &nonce, &record_aad(c.counter), &self.buf)
                        .map_err(|_| std::io::Error::other("chiffrement du journal"))?;
                    c.counter += 1;
                    self.file.write_all(&(ct.len() as u32).to_be_bytes())?;
                    self.file.write_all(&ct)?;
                }
            }
            self.buf.clear();
        }
        self.file.flush()
    }
}

/// Déchiffre un journal `.log.enc` (un bloc final tronqué par un crash est ignoré).
pub fn decrypt_log(path: &Path, dek: &[u8; DEK_LEN]) -> Result<String> {
    let mut f = File::open(path)?;
    let mut head = [0u8; 8 + 16];
    f.read_exact(&mut head)
        .map_err(|_| Error::BadFormat("journal tronqué"))?;
    if &head[..8] != ENC_MAGIC {
        return Err(Error::BadFormat("magic de journal inconnu"));
    }
    let prefix: [u8; 16] = head[8..].try_into().map_err(|_| Error::Crypto)?;
    let key = log_key(dek);
    let mut out = Vec::new();
    let mut counter = 0u64;
    loop {
        let mut len = [0u8; 4];
        if f.read_exact(&mut len).is_err() {
            break;
        }
        let len = u32::from_be_bytes(len) as usize;
        if len > MAX_RECORD {
            return Err(Error::BadFormat("bloc de journal trop volumineux"));
        }
        let mut ct = vec![0u8; len];
        if f.read_exact(&mut ct).is_err() {
            break;
        }
        let pt = crypto::open(
            &key,
            &record_nonce(&prefix, counter),
            &record_aad(counter),
            &ct,
        )?;
        out.extend_from_slice(&pt);
        counter += 1;
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Supprime les journaux plus vieux que `days` jours (0 = conserver). Retourne le nombre supprimé.
pub fn purge_older_than(dir: &Path, days: u32) -> usize {
    if days == 0 {
        return 0;
    }
    let limit = Duration::from_secs(u64::from(days) * 86_400);
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        let is_log = p
            .file_name()
            .and_then(|f| f.to_str())
            .is_some_and(|f| f.ends_with(".log") || f.ends_with(".log.enc"));
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > limit);
        if is_log && old && fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    n
}

/// Fichier de journal d'une session.
pub struct SessionLog {
    path: PathBuf,
    out: Sink,
    stripper: AnsiStripper,
    written: u64,
    cap: u64,
    stopped: bool,
    dirty: bool,
    last_flush: Instant,
}

impl SessionLog {
    /// Crée `<logs>/<nom>-<date>.log` (0600, dossier 0700).
    pub fn create(dd: &DataDir, connection_name: &str) -> Result<Self> {
        Self::create_in(&dd.logs_dir(), connection_name, DEFAULT_CAP)
    }

    /// Variante chiffrée : `dek` fournie → fichier `.log.enc` (sous-clé dérivée de la DEK).
    pub fn create_encrypted(
        dd: &DataDir,
        connection_name: &str,
        dek: &[u8; DEK_LEN],
    ) -> Result<Self> {
        Self::create_in_with(&dd.logs_dir(), connection_name, DEFAULT_CAP, Some(dek))
    }

    pub fn create_in(dir: &Path, connection_name: &str, cap: u64) -> Result<Self> {
        Self::create_in_with(dir, connection_name, cap, None)
    }

    pub fn create_in_with(
        dir: &Path,
        connection_name: &str,
        cap: u64,
        dek: Option<&[u8; DEK_LEN]>,
    ) -> Result<Self> {
        let ext = if dek.is_some() { "log.enc" } else { "log" };
        fs::create_dir_all(dir)?;
        set_private_dir(dir)?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let base = format!("{}-{stamp}", sanitize_name(connection_name));
        let mut n = 0u32;
        let (path, file) = loop {
            let name = if n == 0 {
                format!("{base}.{ext}")
            } else {
                format!("{base}-{n}.{ext}")
            };
            let path = dir.join(name);
            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            match opts.open(&path) {
                Ok(f) => break (path, f),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 100 => n += 1,
                Err(e) => return Err(e.into()),
            }
        };
        set_private_file(&path)?;
        let mut file = file;
        let cipher = match dek {
            Some(d) => {
                let prefix = random_array::<16>()?;
                file.write_all(ENC_MAGIC)?;
                file.write_all(&prefix)?;
                Some(LogCipher {
                    key: log_key(d),
                    prefix,
                    counter: 0,
                })
            }
            None => None,
        };
        let mut log = Self {
            path,
            out: Sink {
                file,
                buf: Vec::new(),
                cipher,
            },
            stripper: AnsiStripper::new(),
            written: 0,
            cap,
            stopped: false,
            dirty: false,
            last_flush: Instant::now(),
        };
        let clean_name: String = connection_name
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let header = format!(
            "# HarakaTS session log - {clean_name} - started {}",
            now_rfc3339()
        );
        log.write_line(&header)?;
        log.flush()?;
        Ok(log)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn bytes_written(&self) -> u64 {
        self.written
    }

    /// Données non encore vidées sur disque.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        if self.stopped {
            return Ok(());
        }
        let len = line.len() as u64 + 1;
        if self.written + len > self.cap {
            self.stopped = true;
            self.out.put(CAP_NOTE)?;
            self.written += CAP_NOTE.len() as u64 + 1;
            return self.flush();
        }
        self.out.put(line)?;
        self.written += len;
        self.dirty = true;
        Ok(())
    }

    /// Ajoute de la sortie terminal décodée (déjà privée des charges OSC 52).
    pub fn write_output(&mut self, text: &str) -> std::io::Result<()> {
        if self.stopped {
            return Ok(());
        }
        let mut lines = Vec::new();
        self.stripper.feed(text, &mut lines);
        for l in lines {
            self.write_line(&l)?;
        }
        if self.dirty && self.last_flush.elapsed() >= FLUSH_INTERVAL {
            self.flush()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()?;
        self.dirty = false;
        self.last_flush = Instant::now();
        Ok(())
    }

    /// Termine : écrit la ligne partielle, vide et retourne la taille écrite.
    pub fn finish(mut self) -> std::io::Result<u64> {
        if let Some(rest) = self.stripper.finish() {
            self.write_line(&rest)?;
        }
        self.flush()?;
        Ok(self.written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(chunks: &[&str]) -> Vec<String> {
        let mut s = AnsiStripper::new();
        let mut out = Vec::new();
        for c in chunks {
            s.feed(c, &mut out);
        }
        out.extend(s.finish());
        out
    }

    #[test]
    fn csi_sgr_and_cursor_removed() {
        assert_eq!(
            strip(&["\x1b[1;31mrouge\x1b[0m ok\x1b[2K\x1b[?25h\n"]),
            vec!["rouge ok"]
        );
    }

    #[test]
    fn osc_bel_and_st_terminated() {
        assert_eq!(strip(&["\x1b]0;titre\x07abc\n"]), vec!["abc"]);
        assert_eq!(
            strip(&["\x1b]8;;http://x\x1b\\lien\x1b]8;;\x1b\\\n"]),
            vec!["lien"]
        );
        // OSC 7 / OSC 52 résiduel
        assert_eq!(strip(&["a\x1b]52;c;SGVsbG8=\x07b\n"]), vec!["ab"]);
    }

    #[test]
    fn charset_selects_and_single_escapes() {
        assert_eq!(
            strip(&["\x1b(B\x1b)0x\x1b7y\x1b8z\x1b=\x1b>\n"]),
            vec!["xyz"]
        );
        assert_eq!(strip(&["\x1b#8e\n"]), vec!["e"]);
    }

    #[test]
    fn stray_esc_does_not_eat_text_or_newline() {
        assert_eq!(strip(&["a\x1b\nb\n"]), vec!["a", "b"]);
        assert_eq!(strip(&["a\x1b\x1b[0mb\n"]), vec!["ab"]);
        assert_eq!(strip(&["fin\x1b"]), vec!["fin"]);
    }

    #[test]
    fn sequences_split_across_chunks() {
        assert_eq!(
            strip(&["a\x1b", "[3", "1mb\x1b]0;ti", "tre\x1b", "\\c\n"]),
            vec!["abc"]
        );
    }

    #[test]
    fn crlf_and_carriage_return_overwrite() {
        assert_eq!(strip(&["a\r\nb\r\n"]), vec!["a", "b"]);
        assert_eq!(strip(&["10%\r50%\r100%\n"]), vec!["100%"]);
        assert_eq!(strip(&["abc\rXY\n"]), vec!["XYc"]);
        assert_eq!(strip(&["ab\x08\x08xy\n"]), vec!["xy"]);
    }

    #[test]
    fn controls_dropped_tab_kept_utf8_kept() {
        assert_eq!(strip(&["a\x07\tb\x00é\x7f\n"]), vec!["a\tbé"]);
    }

    #[test]
    fn unterminated_osc_is_bounded() {
        let long = format!("\x1b]0;{}", "x".repeat(MAX_STRING_LEN + 10));
        let out = strip(&[&long, "visible\n"]);
        assert!(out.last().unwrap().ends_with("visible"));
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_name("web/prod 01"), "web_prod_01");
        assert_eq!(sanitize_name("../../etc"), "etc");
        assert_eq!(sanitize_name("///"), "session");
        assert_eq!(sanitize_name("é-srv"), "é-srv");
    }

    #[cfg(unix)]
    #[test]
    fn log_file_permissions_header_and_content() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        let mut log = SessionLog::create_in(&dir, "mon/srv", DEFAULT_CAP).unwrap();
        let path = log.path().to_path_buf();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        log.write_output("\x1b[32mhello\x1b[0m\r\nworld").unwrap();
        log.flush().unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("# HarakaTS session log - mon/srv - started "));
        assert!(content.contains("\nhello\n"));
        // ligne partielle non écrite avant la fin
        assert!(!content.contains("world"));
        log.finish().unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .ends_with("hello\nworld\n")
        );
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("mon_srv-")
        );
    }

    #[test]
    fn same_second_does_not_clobber() {
        let tmp = tempfile::tempdir().unwrap();
        let a = SessionLog::create_in(tmp.path(), "x", DEFAULT_CAP).unwrap();
        let b = SessionLog::create_in(tmp.path(), "x", DEFAULT_CAP).unwrap();
        assert_ne!(a.path(), b.path());
    }

    #[test]
    fn size_cap_stops_with_note() {
        let tmp = tempfile::tempdir().unwrap();
        let mut log = SessionLog::create_in(tmp.path(), "cap", 200).unwrap();
        for i in 0..50 {
            log.write_output(&format!("line number {i}\n")).unwrap();
        }
        assert!(log.is_stopped());
        let path = log.path().to_path_buf();
        let written = log.finish().unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.trim_end().ends_with(CAP_NOTE), "{content}");
        assert!(content.len() as u64 <= 200 + CAP_NOTE.len() as u64 + 1);
        assert_eq!(content.len() as u64, written);
        assert!(!content.contains("line number 49"));
    }

    #[test]
    fn flush_is_periodic_and_explicit() {
        let tmp = tempfile::tempdir().unwrap();
        let mut log = SessionLog::create_in(tmp.path(), "fl", DEFAULT_CAP).unwrap();
        let path = log.path().to_path_buf();
        log.write_output("un\n").unwrap();
        assert!(log.is_dirty());
        assert!(!fs::read_to_string(&path).unwrap().contains("un"));
        // Forcer l'échéance du vidage périodique.
        log.last_flush = Instant::now() - FLUSH_INTERVAL - Duration::from_millis(10);
        log.write_output("deux\n").unwrap();
        assert!(!log.is_dirty());
        assert!(fs::read_to_string(&path).unwrap().contains("deux"));
    }

    #[test]
    fn encrypted_roundtrip_hides_plaintext_and_detects_tamper() {
        let tmp = tempfile::tempdir().unwrap();
        let dek = [7u8; DEK_LEN];
        let mut log =
            SessionLog::create_in_with(tmp.path(), "srv", DEFAULT_CAP, Some(&dek)).unwrap();
        let path = log.path().to_path_buf();
        assert!(path.to_string_lossy().ends_with(".log.enc"));
        log.write_output("secret-token-123\r\nsecond\r\n").unwrap();
        log.flush().unwrap();
        log.write_output("third\r\n").unwrap();
        log.finish().unwrap();
        let raw = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("secret-token-123"));
        let text = decrypt_log(&path, &dek).unwrap();
        assert!(text.contains("secret-token-123\nsecond\nthird\n"));
        assert!(decrypt_log(&path, &[8u8; DEK_LEN]).is_err());
        // octet modifié : échec d'authentification
        let mut bad = raw.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        let p2 = tmp.path().join("bad.log.enc");
        fs::write(&p2, bad).unwrap();
        assert!(decrypt_log(&p2, &dek).is_err());
        // bloc final tronqué (crash) : les blocs complets restent lisibles
        let p3 = tmp.path().join("cut.log.enc");
        fs::write(&p3, &raw[..raw.len() - 3]).unwrap();
        assert!(decrypt_log(&p3, &dek).unwrap().contains("secret-token-123"));
    }

    #[test]
    fn purge_keeps_recent_and_ignores_other_files() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.log");
        let other = tmp.path().join("notes.txt");
        fs::write(&a, "x").unwrap();
        fs::write(&other, "x").unwrap();
        assert_eq!(purge_older_than(tmp.path(), 30), 0);
        assert_eq!(purge_older_than(tmp.path(), 0), 0);
        assert!(a.exists() && other.exists());
    }
}
