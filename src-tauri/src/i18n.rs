//! Textes de l'interface NATIVE (fenêtres GTK+VTE : menus, boutons, dialogues,
//! barre d'état, messages affichés dans le terminal).
//!
//! Source unique avec l'interface web : les mêmes fichiers JSON
//! `ui/src/locales/<code>.json` (clés `native.*`), embarqués à la compilation.
//! Résolution : langue demandée → anglais → français (référence) → clé.
//! Ajouter une langue : déposer son JSON dans `ui/src/locales/` puis ajouter une
//! ligne dans `LOCALES` — le test `all_locale_files_are_registered` échoue
//! sinon.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Langues embarquées (code, contenu JSON).
const LOCALES: &[(&str, &str)] = &[
    ("fr", include_str!("../../ui/src/locales/fr.json")),
    ("en", include_str!("../../ui/src/locales/en.json")),
    ("it", include_str!("../../ui/src/locales/it.json")),
    ("de", include_str!("../../ui/src/locales/de.json")),
    ("es", include_str!("../../ui/src/locales/es.json")),
    ("pt", include_str!("../../ui/src/locales/pt.json")),
];

const REFERENCE: &str = "fr";
const FALLBACK: &str = "en";

type Catalog = HashMap<String, String>;

fn catalogs() -> &'static HashMap<&'static str, Catalog> {
    static CATALOGS: OnceLock<HashMap<&'static str, Catalog>> = OnceLock::new();
    CATALOGS.get_or_init(|| {
        LOCALES
            .iter()
            .filter_map(|(code, json)| {
                match serde_json::from_str::<Catalog>(json) {
                    Ok(cat) => Some((*code, cat)),
                    Err(e) => {
                        // Un JSON invalide ne doit pas empêcher l'application de démarrer.
                        tracing::error!(locale = code, "catalogue de langue invalide : {e}");
                        None
                    }
                }
            })
            .collect()
    })
}

fn lookup(locale: &str, key: &str) -> Option<&'static str> {
    let cats = catalogs();
    cats.get(locale)
        .and_then(|c| c.get(key))
        .or_else(|| cats.get(FALLBACK).and_then(|c| c.get(key)))
        .or_else(|| cats.get(REFERENCE).and_then(|c| c.get(key)))
        .map(String::as_str)
}

/// Traduit une clé (la clé elle-même si elle est inconnue).
pub fn tr(locale: &str, key: &str) -> String {
    lookup(locale, key).unwrap_or(key).to_string()
}

/// Traduit une clé et remplace les paramètres `{nom}`.
pub fn tr_with(locale: &str, key: &str, params: &[(&str, &str)]) -> String {
    let mut text = tr(locale, key);
    for (name, value) in params {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_locale_files_are_registered() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/src/locales");
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".json").map(str::to_string)
            })
            .collect();
        files.sort();
        let mut registered: Vec<String> = LOCALES.iter().map(|(c, _)| c.to_string()).collect();
        registered.sort();
        assert_eq!(
            files, registered,
            "chaque ui/src/locales/<code>.json doit figurer dans LOCALES (src-tauri/src/i18n.rs)"
        );
    }

    #[test]
    fn every_catalog_parses_and_has_native_keys() {
        let cats = catalogs();
        assert_eq!(cats.len(), LOCALES.len(), "JSON invalide dans un catalogue");
        let reference = &cats[REFERENCE];
        let native: Vec<&String> = reference
            .keys()
            .filter(|k| k.starts_with("native."))
            .collect();
        assert!(!native.is_empty());
        for (code, cat) in cats {
            for key in &native {
                assert!(cat.contains_key(*key), "{code}: clé {key} manquante");
            }
        }
    }

    #[test]
    fn fallback_and_params() {
        assert_eq!(tr("xx", "native.menu.copy"), tr("en", "native.menu.copy"));
        assert_eq!(tr("fr", "clé.inconnue"), "clé.inconnue");
        let s = tr_with("fr", "native.status.closed", &[("reason", "test-42")]);
        assert!(s.contains("test-42"), "{s}");
    }
}
