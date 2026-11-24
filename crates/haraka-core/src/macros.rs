//! Envoi de macros : conversion du contenu (multi-lignes) en frappes clavier.
//!
//! Règle : le contenu est envoyé « comme tapé ». Chaque fin de ligne du
//! contenu (`\n`, `\r\n` ou `\r`) devient UNE frappe Entrée (`\r`), y compris
//! pour la dernière ligne si le contenu se termine par un saut de ligne. Sans
//! saut de ligne final, la dernière ligne est tapée mais pas validée (cas d'un
//! début de commande à compléter). Aucun encadrement « bracketed paste » : ce
//! sont des frappes, pas un collage.

/// Découpe le contenu en lignes (séparateurs `\n`, `\r\n`, `\r`). Un saut de
/// ligne final ne produit pas de ligne vide supplémentaire.
pub fn split_lines(content: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                lines.push(&content[start..i]);
                start = i + 1;
            }
            b'\r' => {
                lines.push(&content[start..i]);
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < content.len() {
        lines.push(&content[start..]);
    }
    lines
}

/// Frappes à envoyer : lignes jointes par `\r` (Entrée).
pub fn to_keystrokes(content: &str) -> String {
    let mut out = String::with_capacity(content.len() + 1);
    let lines = split_lines(content);
    let ends_with_newline = content.ends_with('\n') || content.ends_with('\r');
    for (i, line) in lines.iter().enumerate() {
        out.push_str(line);
        if i + 1 < lines.len() || ends_with_newline {
            out.push('\r');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_all_newline_flavours() {
        assert_eq!(split_lines("a\nb\r\nc\rd"), vec!["a", "b", "c", "d"]);
        assert_eq!(split_lines("a\n"), vec!["a"]);
        assert_eq!(split_lines("a\n\nb"), vec!["a", "", "b"]);
        assert!(split_lines("").is_empty());
    }

    #[test]
    fn keystrokes_use_cr_and_keep_trailing_state() {
        assert_eq!(to_keystrokes("ls"), "ls");
        assert_eq!(to_keystrokes("ls\n"), "ls\r");
        assert_eq!(to_keystrokes("cd /tmp\r\npwd\n"), "cd /tmp\rpwd\r");
        assert_eq!(to_keystrokes("a\n\nb"), "a\r\rb");
        assert!(!to_keystrokes("x\ny\n").contains('\n'));
    }

    #[test]
    fn utf8_and_controls_pass_through() {
        assert_eq!(to_keystrokes("écho é\n"), "écho é\r");
        assert_eq!(to_keystrokes("\x1b[A\n"), "\x1b[A\r");
    }
}
