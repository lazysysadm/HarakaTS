//! OSC 52 (manipulation du presse-papier par le distant) — jalon M4.
//!
//! Intercepté **dans le backend**, avant le terminal (VTE natif ou xterm) :
//! les séquences OSC 52 n'atteignent jamais le renderer, quelle que soit son
//! support natif. Deux opérations :
//! - `Write` (r2l) : le distant écrit dans le presse-papier local ;
//! - `Query` (l2r) : le distant **lit** le presse-papier local (exfiltration
//!   potentielle — soumis à politique + audité).
//!
//! L'extracteur est streaming : une séquence coupée entre deux chunks est
//! retenue (carry) puis complétée ; les autres séquences d'échappement
//! passent intactes.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

/// Politique presse-papier effective (héritage system → dossiers → connexion).
/// `r2l` = distant → local (OSC 52 write) ; `l2r` = local → distant (query).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipboardPolicy {
    pub r2l: bool,
    pub l2r: bool,
}

pub fn parse_policy(s: &str) -> ClipboardPolicy {
    match s {
        "disabled" => ClipboardPolicy {
            r2l: false,
            l2r: false,
        },
        "l2r" => ClipboardPolicy {
            r2l: false,
            l2r: true,
        },
        "r2l" => ClipboardPolicy {
            r2l: true,
            l2r: false,
        },
        // "bidirectional" + toute valeur inconnue → défaut permissif historique
        _ => ClipboardPolicy {
            r2l: true,
            l2r: true,
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Osc52Op {
    Write { clipboard: String, data: Vec<u8> },
    Query { clipboard: String },
}

const PREFIX: &str = "\x1b]52;";

enum State {
    Normal,
    Payload { over: bool },
}

/// Extracteur streaming OSC 52. `max_bytes` = taille maximale du contenu
/// décodé accepté (au-delà, la séquence est supprimée sans opération).
pub struct Osc52Extractor {
    state: State,
    carry: String,
    payload: String,
    max_b64: usize,
    max_bytes: usize,
}

impl Osc52Extractor {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            state: State::Normal,
            carry: String::new(),
            payload: String::new(),
            // Garde mémoire : base64 ~4/3 de la taille décodée + marge.
            // Le refus contractuel se fait sur la taille DÉCODÉE (take_payload).
            max_b64: max_bytes.saturating_mul(2).saturating_add(64),
            max_bytes,
        }
    }

    /// Retourne (texte nettoyé, opérations). Le texte nettoyé exclut les
    /// séquences OSC 52 interceptées ; tout le reste est préservé à l'identique.
    pub fn push(&mut self, text: &str) -> (String, Vec<Osc52Op>) {
        let input = if self.carry.is_empty() {
            text.to_string()
        } else {
            let mut s = std::mem::take(&mut self.carry);
            s.push_str(text);
            s
        };
        let mut out = String::with_capacity(input.len());
        let mut ops = Vec::new();
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            match self.state {
                State::Normal => {
                    if c == '\x1b' {
                        let rest: String = chars.clone().take(PREFIX.len() - 1).collect();
                        if rest.len() < PREFIX.len() - 1 && PREFIX[1..].starts_with(&rest) {
                            // préfixe incomplet en fin de chunk → on retient
                            self.carry = format!("\x1b{rest}");
                            break;
                        }
                        if rest.len() == PREFIX.len() - 1 && PREFIX[1..] == rest {
                            for _ in 0..PREFIX.len() - 1 {
                                chars.next();
                            }
                            self.payload.clear();
                            self.state = State::Payload { over: false };
                        } else {
                            // autre séquence d'échappement → passe intacte
                            out.push(c);
                        }
                    } else {
                        out.push(c);
                    }
                }
                State::Payload { over } => {
                    if c == '\x07' {
                        if let Some(op) = self.take_payload(over) {
                            ops.push(op);
                        }
                        self.state = State::Normal;
                    } else if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        if let Some(op) = self.take_payload(over) {
                            ops.push(op);
                        }
                        self.state = State::Normal;
                    } else if c == '\x1b' {
                        // ESC inattendu dans la charge → séquence invalide,
                        // on scanne jusqu'au terminateur sans rien émettre.
                        self.state = State::Payload { over: true };
                    } else if !over {
                        if self.payload.len() < self.max_b64 {
                            self.payload.push(c);
                        } else {
                            self.state = State::Payload { over: true };
                        }
                    }
                }
            }
        }
        (out, ops)
    }

    /// Fin du flux : restitue le carry (séquence incomplète non interceptée).
    pub fn finish(&mut self) -> String {
        let mut out = std::mem::take(&mut self.carry);
        if matches!(self.state, State::Payload { .. }) {
            out.push_str(PREFIX);
            out.push_str(&self.payload);
            self.payload.clear();
            self.state = State::Normal;
        }
        out
    }

    fn take_payload(&mut self, over: bool) -> Option<Osc52Op> {
        let payload = std::mem::take(&mut self.payload);
        if over {
            return None;
        }
        let (sel, data) = payload.split_once(';')?;
        let clipboard = if sel.is_empty() {
            "c".to_string()
        } else {
            sel.to_string()
        };
        if data == "?" {
            return Some(Osc52Op::Query { clipboard });
        }
        let bytes = B64.decode(data).ok()?;
        if bytes.len() > self.max_bytes {
            return None;
        }
        Some(Osc52Op::Write {
            clipboard,
            data: bytes,
        })
    }
}

/// Réponse à une query OSC 52 (l2r) : renvoie le contenu au distant.
pub fn query_response(clipboard: &str, data: &[u8]) -> String {
    format!("\x1b]52;{clipboard};{}\x07", B64.encode(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_op(data: &[u8]) -> Osc52Op {
        Osc52Op::Write {
            clipboard: "c".into(),
            data: data.to_vec(),
        }
    }

    #[test]
    fn policy_matrix() {
        assert_eq!(
            parse_policy("disabled"),
            ClipboardPolicy {
                r2l: false,
                l2r: false
            }
        );
        assert_eq!(
            parse_policy("l2r"),
            ClipboardPolicy {
                r2l: false,
                l2r: true
            }
        );
        assert_eq!(
            parse_policy("r2l"),
            ClipboardPolicy {
                r2l: true,
                l2r: false
            }
        );
        assert_eq!(
            parse_policy("bidirectional"),
            ClipboardPolicy {
                r2l: true,
                l2r: true
            }
        );
        assert_eq!(
            parse_policy("inconnu"),
            ClipboardPolicy {
                r2l: true,
                l2r: true
            }
        );
    }

    #[test]
    fn write_bel_terminator() {
        let mut ex = Osc52Extractor::new(100_000);
        let (out, ops) = ex.push("\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(out, "");
        assert_eq!(ops, vec![write_op(b"hello")]);
    }

    #[test]
    fn write_st_terminator() {
        let mut ex = Osc52Extractor::new(100_000);
        let (out, ops) = ex.push("\x1b]52;c;aGVsbG8=\x1b\\");
        assert_eq!(out, "");
        assert_eq!(ops, vec![write_op(b"hello")]);
    }

    #[test]
    fn query() {
        let mut ex = Osc52Extractor::new(100_000);
        let (out, ops) = ex.push("\x1b]52;c;?\x07");
        assert_eq!(out, "");
        assert_eq!(
            ops,
            vec![Osc52Op::Query {
                clipboard: "c".into()
            }]
        );
    }

    #[test]
    fn other_sequences_pass_through() {
        let mut ex = Osc52Extractor::new(100_000);
        let input = "abc\x1b[31mrouge\x1b[0m\x1b]0;titre\x07fin";
        let (out, ops) = ex.push(input);
        assert_eq!(out, input);
        assert!(ops.is_empty());
    }

    #[test]
    fn split_prefix_across_chunks() {
        let mut ex = Osc52Extractor::new(100_000);
        let (out1, ops1) = ex.push("texte\x1b]5");
        assert_eq!(out1, "texte");
        assert!(ops1.is_empty());
        let (out2, ops2) = ex.push("2;c;aGk=\x07suite");
        assert_eq!(out2, "suite");
        assert_eq!(ops2, vec![write_op(b"hi")]);
    }

    #[test]
    fn split_payload_across_chunks() {
        let mut ex = Osc52Extractor::new(100_000);
        let (_, ops1) = ex.push("\x1b]52;c;aGVs");
        assert!(ops1.is_empty());
        let (out, ops2) = ex.push("bG8=\x07");
        assert_eq!(out, "");
        assert_eq!(ops2, vec![write_op(b"hello")]);
    }

    #[test]
    fn oversize_dropped() {
        let mut ex = Osc52Extractor::new(4);
        let data = B64.encode(b"0123456789abcdef");
        let (out, ops) = ex.push(&format!("\x1b]52;c;{data}\x07"));
        assert_eq!(out, "");
        assert!(ops.is_empty(), "OSC 52 surdimensionné doit être refusé");
    }

    #[test]
    fn surrounding_text_preserved() {
        let mut ex = Osc52Extractor::new(100_000);
        let (out, ops) = ex.push("avant\x1b]52;c;YQ==\x07après");
        assert_eq!(out, "avantaprès");
        assert_eq!(ops, vec![write_op(b"a")]);
    }

    #[test]
    fn empty_selection_defaults_to_c() {
        let mut ex = Osc52Extractor::new(100_000);
        let (_, ops) = ex.push("\x1b]52;;YQ==\x07");
        assert_eq!(ops, vec![write_op(b"a")]);
    }

    #[test]
    fn invalid_base64_dropped() {
        let mut ex = Osc52Extractor::new(100_000);
        let (out, ops) = ex.push("\x1b]52;c;!!!pas-du-base64!!!\x07");
        assert_eq!(out, "");
        assert!(ops.is_empty());
    }

    #[test]
    fn esc_inside_payload_invalidates() {
        let mut ex = Osc52Extractor::new(100_000);
        let (out, ops) = ex.push("\x1b]52;c;YQ==\x1b[31mX\x07");
        assert_eq!(out, "");
        assert!(ops.is_empty());
    }

    #[test]
    fn finish_flushes_carry() {
        let mut ex = Osc52Extractor::new(100_000);
        let (out, _) = ex.push("fin\x1b]5");
        assert_eq!(out, "fin");
        assert_eq!(ex.finish(), "\x1b]5");
    }

    #[test]
    fn query_response_roundtrip() {
        let resp = query_response("c", b"secret-local");
        let mut ex = Osc52Extractor::new(100_000);
        let (_, ops) = ex.push(&resp);
        assert_eq!(ops, vec![write_op(b"secret-local")]);
    }
}
