//! Coloration de la sortie terminal (à la MobaXterm) : insère des séquences SGR
//! autour de mots-clés (erreurs, avertissements, succès, IP, URL) dans le
//! flux affiché. Le journal de session reste brut (appelé APRÈS la journalisation).
//!
//! Garde-fous : jamais dans l'écran alternatif (vim, htop, less, tmux), jamais
//! dans une séquence d'échappement, et jamais quand le serveur a déjà posé une
//! couleur de premier plan (`ls --color`, `grep --color`…).

use std::sync::LazyLock;

use regex::Regex;

const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";
const RESET_FG: &str = "\x1b[39m";

/// Règles fixes, par priorité décroissante (une plage déjà prise n'est pas recolorée).
static RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let r = |p: &str, c: &'static str| (Regex::new(p).expect("regex de coloration"), c);
    vec![
        r(r"https?://[^\s\x1b]+", CYAN),
        r(r"\b\d{1,3}(?:\.\d{1,3}){3}\b", CYAN),
        r(
            r"(?i)\b(?:errors?|failed|failures?|fatal|critical|panic|denied|refused|segfault|exceptions?|unreachable)\b",
            RED,
        ),
        r(
            r"(?i)\b(?:warn|warnings?|deprecated|timeout|timed out)\b",
            YELLOW,
        ),
        r(
            r"(?i)\b(?:success|successful|successfully|ok|passed|enabled|completed)\b|active \(running\)",
            GREEN,
        ),
    ]
});

#[derive(Clone, Copy, PartialEq, Eq)]
enum Esc {
    Ground,
    /// Après ESC.
    Escape,
    /// Dans une séquence CSI (ESC [ …).
    Csi,
    /// Chaîne OSC / DCS / APC… jusqu'à BEL ou ST.
    Str,
    /// Dans une chaîne, après un ESC (candidat ST).
    StrTerm,
}

pub struct Highlighter {
    enabled: bool,
    state: Esc,
    /// Paramètres CSI en cours d'accumulation.
    csi: String,
    alt_screen: bool,
    /// Le serveur a posé une couleur de premier plan : ne rien ajouter.
    fg_set: bool,
}

impl Highlighter {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            state: Esc::Ground,
            csi: String::new(),
            alt_screen: false,
            fg_set: false,
        }
    }

    /// Texte à afficher : `text` coloré, ou inchangé si rien à faire.
    pub fn apply(&mut self, text: &str) -> String {
        if !self.enabled {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len() + 16);
        let mut plain_start: Option<usize> = None;
        for (i, ch) in text.char_indices() {
            match self.state {
                Esc::Ground => {
                    if ch == '\x1b' {
                        if let Some(s) = plain_start.take() {
                            self.flush_plain(&text[s..i], &mut out);
                        }
                        out.push(ch);
                        self.state = Esc::Escape;
                    } else if plain_start.is_none() {
                        plain_start = Some(i);
                    }
                }
                Esc::Escape => {
                    out.push(ch);
                    self.state = match ch {
                        '[' => {
                            self.csi.clear();
                            Esc::Csi
                        }
                        ']' | 'P' | '_' | '^' | 'X' => Esc::Str,
                        _ => Esc::Ground,
                    };
                }
                Esc::Csi => {
                    out.push(ch);
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        self.end_csi(ch);
                        self.state = Esc::Ground;
                    } else {
                        self.csi.push(ch);
                    }
                }
                Esc::Str => {
                    out.push(ch);
                    match ch {
                        '\x07' => self.state = Esc::Ground,
                        '\x1b' => self.state = Esc::StrTerm,
                        _ => {}
                    }
                }
                Esc::StrTerm => {
                    out.push(ch);
                    self.state = if ch == '\\' { Esc::Ground } else { Esc::Str };
                }
            }
        }
        if let Some(s) = plain_start {
            self.flush_plain(&text[s..], &mut out);
        }
        out
    }

    fn flush_plain(&self, run: &str, out: &mut String) {
        if self.alt_screen || self.fg_set {
            out.push_str(run);
        } else {
            colorize(run, out);
        }
    }

    /// Fin d'une CSI : suit l'écran alternatif et la couleur de premier plan.
    fn end_csi(&mut self, fin: char) {
        match fin {
            'h' | 'l' if self.csi.starts_with('?') => {
                if self.csi[1..]
                    .split(';')
                    .any(|p| matches!(p, "47" | "1047" | "1049"))
                {
                    self.alt_screen = fin == 'h';
                }
            }
            'm' if !self.csi.starts_with(['?', '>', '<', '=']) => {
                if self.csi.is_empty() {
                    self.fg_set = false;
                    return;
                }
                let mut it = self.csi.split([';', ':']).peekable();
                while let Some(p) = it.next() {
                    match p {
                        "" | "0" | "39" => self.fg_set = false,
                        "38" => {
                            self.fg_set = true;
                            // Saute les arguments de 38;5;n / 38;2;r;g;b.
                            match it.next() {
                                Some("5") => {
                                    it.next();
                                }
                                Some("2") => {
                                    for _ in 0..3 {
                                        it.next();
                                    }
                                }
                                _ => {}
                            }
                        }
                        "48" | "58" => match it.next() {
                            Some("5") => {
                                it.next();
                            }
                            Some("2") => {
                                for _ in 0..3 {
                                    it.next();
                                }
                            }
                            _ => {}
                        },
                        n => {
                            if let Ok(n) = n.parse::<u32>()
                                && (matches!(n, 30..=37) || matches!(n, 90..=97))
                            {
                                self.fg_set = true;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Colore un segment de texte brut (sans séquence d'échappement).
fn colorize(run: &str, out: &mut String) {
    let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
    for (re, color) in RULES.iter() {
        for m in re.find_iter(run) {
            let (s, e) = (m.start(), m.end());
            if spans.iter().all(|&(a, b, _)| e <= a || s >= b) {
                spans.push((s, e, color));
            }
        }
    }
    if spans.is_empty() {
        out.push_str(run);
        return;
    }
    spans.sort_by_key(|&(s, _, _)| s);
    let mut pos = 0;
    for (s, e, color) in spans {
        out.push_str(&run[pos..s]);
        out.push_str(color);
        out.push_str(&run[s..e]);
        out.push_str(RESET_FG);
        pos = e;
    }
    out.push_str(&run[pos..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hl(text: &str) -> String {
        Highlighter::new(true).apply(text)
    }

    #[test]
    fn colors_levels_ip_url() {
        assert_eq!(hl("an ERROR here"), "an \x1b[31mERROR\x1b[39m here");
        assert_eq!(hl("warning: x"), "\x1b[33mwarning\x1b[39m: x");
        assert_eq!(hl("Done: ok"), "Done: \x1b[32mok\x1b[39m");
        assert_eq!(hl("to 10.0.0.1 now"), "to \x1b[36m10.0.0.1\x1b[39m now");
        assert!(hl("see https://a.b/c?d=1 ok").contains("\x1b[36mhttps://a.b/c?d=1\x1b[39m"));
        // Les dates et heures restent sans coloration.
        assert_eq!(hl("2026-11-21 10:15:03 up"), "2026-11-21 10:15:03 up");
    }

    #[test]
    fn plain_text_and_words_inside_words_untouched() {
        assert_eq!(hl("hello world"), "hello world");
        assert_eq!(hl("token book"), "token book");
        assert_eq!(hl("terrorism"), "terrorism");
    }

    #[test]
    fn disabled_is_identity() {
        assert_eq!(Highlighter::new(false).apply("ERROR"), "ERROR");
    }

    #[test]
    fn existing_escape_sequences_are_preserved() {
        // OSC titre contenant "error" : intact.
        let osc = "\x1b]0;error title\x07";
        assert_eq!(hl(osc), osc);
        // CSI conservée, texte suivant coloré.
        assert_eq!(hl("\x1b[1mfailed"), "\x1b[1m\x1b[31mfailed\x1b[39m");
    }

    #[test]
    fn server_foreground_color_disables_until_reset() {
        let mut h = Highlighter::new(true);
        assert_eq!(h.apply("\x1b[32mgrep error"), "\x1b[32mgrep error");
        assert_eq!(h.apply("\x1b[0m error"), "\x1b[0m \x1b[31merror\x1b[39m");
        // 256 couleurs et truecolor.
        let mut h = Highlighter::new(true);
        assert_eq!(h.apply("\x1b[38;5;196merror"), "\x1b[38;5;196merror");
        let mut h = Highlighter::new(true);
        assert_eq!(h.apply("\x1b[38;2;1;2;3merror"), "\x1b[38;2;1;2;3merror");
    }

    #[test]
    fn alternate_screen_is_never_colored() {
        let mut h = Highlighter::new(true);
        assert_eq!(h.apply("\x1b[?1049herror"), "\x1b[?1049herror");
        assert_eq!(h.apply("error"), "error");
        assert_eq!(
            h.apply("\x1b[?1049lerror"),
            "\x1b[?1049l\x1b[31merror\x1b[39m"
        );
    }

    #[test]
    fn escape_split_across_chunks() {
        let mut h = Highlighter::new(true);
        let a = h.apply("x\x1b[3");
        let b = h.apply("2merror");
        assert_eq!(format!("{a}{b}"), "x\x1b[32merror");
    }

    #[test]
    fn overlapping_matches_keep_first_priority() {
        // L'IP dans une URL reste dans la plage URL (pas de double coloration).
        assert_eq!(hl("http://10.0.0.1/x"), "\x1b[36mhttp://10.0.0.1/x\x1b[39m");
    }
}
