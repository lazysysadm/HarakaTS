//! Détection de collage à risque (jalon M4) : multiligne ou contenu suspect
//! (caractères de contrôle / échappements hors bracketed paste).
//!
//! Miroir TypeScript : `ui/src/lib/paste.ts` (tests vitest) — garder les deux
//! logiques synchronisées. La confirmation s'applique au collage, pas à la
//! frappe : un Entrée seul (`\r`) ne déclenche rien.

/// true si le texte doit être confirmé avant envoi (réglage
/// `paste_confirm_multiline`). Seuls les marqueurs bracketed paste EN BORDURE
/// sont retirés : un marqueur interne termine le collage prématurément côté
/// terminal et le reste s'exécute comme des frappes (pastejacking) → suspect.
pub fn paste_needs_confirm(text: &str) -> bool {
    let mut stripped = text;
    if let Some(rest) = stripped.strip_prefix("\x1b[200~") {
        stripped = rest;
    }
    if let Some(rest) = stripped.strip_suffix("\x1b[201~") {
        stripped = rest;
    }
    if stripped.contains('\n') || stripped.matches('\r').count() >= 2 {
        return true;
    }
    stripped
        .chars()
        .any(|c| c.is_control() && c != '\r' && c != '\n' && c != '\t')
}

/// Aperçu exact et imprimable : les caractères de contrôle sont rendus
/// visibles (␛ ␍ ␊ ␡) pour que l'utilisateur voie ce qui sera envoyé.
pub fn paste_preview(text: &str) -> String {
    text.replace('\x1b', "␛")
        .replace('\r', "␍")
        .replace('\n', "␊\n")
        .replace('\x7f', "␡")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_enter_and_plain_text_pass() {
        assert!(!paste_needs_confirm("\r"));
        assert!(!paste_needs_confirm("ls -la"));
        assert!(!paste_needs_confirm("echo\thello"));
    }

    #[test]
    fn bracketed_single_line_passes() {
        assert!(!paste_needs_confirm("\x1b[200~hello\x1b[201~"));
    }

    #[test]
    fn multiline_triggers() {
        assert!(paste_needs_confirm("a\r\nb"));
        assert!(paste_needs_confirm("a\rb\rc"));
        assert!(paste_needs_confirm("\x1b[200~a\nb\x1b[201~"));
    }

    #[test]
    fn control_chars_trigger() {
        assert!(paste_needs_confirm("\x1b[31mrouge\x1b[0m"));
        assert!(paste_needs_confirm("a\x00b"));
        assert!(paste_needs_confirm("a\x7fb"));
    }

    #[test]
    fn pastejacking_after_bracketed_end_triggers() {
        // séquence d'échappement cachée après le marqueur de fin
        assert!(paste_needs_confirm(
            "\x1b[200~ok\x1b[201~\x1b[201~rm -rf /\r"
        ));
    }

    #[test]
    fn preview_shows_controls() {
        let p = paste_preview("a\x1b[31mb\r\nc");
        assert!(p.contains("␛[31m"));
        assert!(p.contains("␍"));
        assert!(p.contains("␊"));
    }
}
