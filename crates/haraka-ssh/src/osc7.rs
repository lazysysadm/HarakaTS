//! Détection OSC 7 (`ESC ] 7 ; file://hôte/chemin BEL|ST`) : le shell distant
//! annonce son répertoire courant ; le panneau SFTP peut le suivre (M5).
//!
//! Le scanner gère les séquences coupées entre deux lots de sortie (carry
//! borné). Il ne modifie pas le flux : VTE reçoit la séquence telle quelle.

const PREFIX: &str = "\x1b]7;";
/// Longueur max d'une séquence retenue d'un lot au suivant.
const MAX_CARRY: usize = 4096;

#[derive(Default)]
pub struct Osc7Scanner {
    carry: String,
    first_end: Option<usize>,
}

impl Osc7Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Position (octets, relative au dernier lot passé à `push`) de la fin de la
    /// PREMIÈRE séquence OSC 7 complète de ce lot, valide ou non. Sert à insérer
    /// du texte juste après elle (masquage de la ligne d'activation du suivi).
    pub fn first_end(&self) -> Option<usize> {
        self.first_end
    }

    /// Consomme un lot ; retourne le DERNIER répertoire annoncé, s'il y en a un.
    pub fn push(&mut self, chunk: &str) -> Option<String> {
        let mut text = std::mem::take(&mut self.carry);
        let carry_len = text.len();
        text.push_str(chunk);
        self.first_end = None;
        let mut last = None;
        let mut rest = text.as_str();
        while let Some(start) = rest.find(PREFIX) {
            let body = &rest[start + PREFIX.len()..];
            match find_terminator(body) {
                Some((end, term_len)) => {
                    if let Some(path) = parse_uri(&body[..end]) {
                        last = Some(path);
                    }
                    let rest_after = &body[end + term_len..];
                    if self.first_end.is_none() {
                        // fin absolue dans `text`, ramenée aux coordonnées du lot
                        self.first_end =
                            Some((text.len() - rest_after.len()).saturating_sub(carry_len));
                    }
                    rest = rest_after;
                }
                None => {
                    // Séquence incomplète : on la garde pour le lot suivant.
                    let pending = &rest[start..];
                    if pending.len() <= MAX_CARRY {
                        self.carry = pending.to_string();
                    }
                    return last;
                }
            }
        }
        // Un préfixe peut être coupé en fin de lot (ex. « ESC ] »).
        for cut in (1..PREFIX.len()).rev() {
            if rest.len() >= cut && rest.is_char_boundary(rest.len() - cut) {
                let tail = &rest[rest.len() - cut..];
                if PREFIX.starts_with(tail) {
                    self.carry = tail.to_string();
                    break;
                }
            }
        }
        last
    }
}

/// (index de fin du corps, longueur du terminateur) : BEL ou ST (`ESC \`).
fn find_terminator(body: &str) -> Option<(usize, usize)> {
    let bel = body.find('\x07');
    let st = body.find("\x1b\\");
    match (bel, st) {
        (Some(b), Some(s)) if b < s => Some((b, 1)),
        (Some(_), Some(s)) => Some((s, 2)),
        (Some(b), None) => Some((b, 1)),
        (None, Some(s)) => Some((s, 2)),
        (None, None) => None,
    }
}

/// `file://hôte/chemin%20encodé` → `/chemin encodé` (hôte ignoré).
fn parse_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let decoded = percent_decode(path)?;
    decoded.starts_with('/').then_some(decoded)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bel_and_st_terminators() {
        let mut s = Osc7Scanner::new();
        assert_eq!(
            s.push("a\x1b]7;file://host/home/u\x07b").as_deref(),
            Some("/home/u")
        );
        assert_eq!(
            s.push("\x1b]7;file:///tmp/x\x1b\\").as_deref(),
            Some("/tmp/x")
        );
    }

    #[test]
    fn percent_decoding_and_last_wins() {
        let mut s = Osc7Scanner::new();
        let got = s.push("\x1b]7;file://h/a%20b\x07..\x1b]7;file://h/c%C3%A9\x07");
        assert_eq!(got.as_deref(), Some("/cé"));
    }

    #[test]
    fn split_across_chunks() {
        let mut s = Osc7Scanner::new();
        assert_eq!(s.push("x\x1b]7;file://ho"), None);
        assert_eq!(s.push("st/var/log\x07").as_deref(), Some("/var/log"));
        assert_eq!(s.push("\x1b"), None);
        assert_eq!(s.push("]7;file:///opt\x07").as_deref(), Some("/opt"));
    }

    #[test]
    fn first_end_points_after_first_sequence() {
        let mut s = Osc7Scanner::new();
        let chunk = "ab\x1b]7;file://h/x\x07PROMPT\x1b]7;file://h/y\x07";
        s.push(chunk);
        let end = s.first_end().unwrap();
        assert_eq!(&chunk[..end], "ab\x1b]7;file://h/x\x07");
        // séquence coupée entre deux lots : fin dans le second lot
        let mut s = Osc7Scanner::new();
        s.push("x\x1b]7;file://h");
        assert_eq!(s.first_end(), None);
        let second = "/z\x07rest";
        s.push(second);
        assert_eq!(&second[..s.first_end().unwrap()], "/z\x07");
    }

    #[test]
    fn ignores_garbage() {
        let mut s = Osc7Scanner::new();
        assert_eq!(s.push("\x1b]7;http://x/y\x07"), None);
        assert_eq!(s.push("\x1b]7;file://h/%zz\x07"), None);
        assert_eq!(s.push("plain text"), None);
    }
}
