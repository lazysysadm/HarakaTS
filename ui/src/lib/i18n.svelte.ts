// Internationalisation : un fichier JSON par langue dans `src/locales/`.
//
// Ajouter une langue = déposer `src/locales/<code>.json` (copie de `fr.json`
// traduite, avec la clé `_name` = nom de la langue dans sa propre langue) ;
// elle est découverte automatiquement (import.meta.glob) et apparaît dans
// Réglages → Langue. Côté application native (GTK/VTE), le même fichier est
// embarqué par `src-tauri/src/i18n.rs` (clés `native.*`) : un test vérifie que
// toutes les langues y sont déclarées.
//
// Résolution d'une clé : langue active → anglais → français (référence) → clé.
// Paramètres : `{nom}` ; pluriels : clés `<clé>_one` / `<clé>_other` via `tn`.

export type Catalog = Record<string, string>;

const modules = import.meta.glob("../locales/*.json", {
  eager: true,
  import: "default",
}) as Record<string, Catalog>;

/** Catalogues par code de langue (« fr », « en », « it »…). */
export const catalogs: Record<string, Catalog> = Object.fromEntries(
  Object.entries(modules).map(([path, cat]) => [path.replace(/^.*\/([^/]+)\.json$/, "$1"), cat]),
);

/** Langue de référence : toutes les clés y existent. */
export const REFERENCE_LOCALE = "fr";
const FALLBACK_LOCALE = "en";
const STORAGE_KEY = "harakats.locale";

export function availableLocales(): { code: string; name: string }[] {
  return Object.entries(catalogs)
    .map(([code, cat]) => ({ code, name: cat._name ?? code }))
    .sort((a, b) => (a.code === REFERENCE_LOCALE ? -1 : b.code === REFERENCE_LOCALE ? 1 : a.name.localeCompare(b.name)));
}

function isSupported(code: string | null | undefined): code is string {
  return !!code && code in catalogs;
}

/** Langue initiale (avant déverrouillage du coffre) : choix mémorisé, sinon langue du système. */
function detectInitialLocale(): string {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (isSupported(saved)) return saved;
  } catch {
    // stockage indisponible (mode privé…) : on continue
  }
  const nav = typeof navigator !== "undefined" ? navigator.language?.slice(0, 2).toLowerCase() : undefined;
  return isSupported(nav) ? nav : REFERENCE_LOCALE;
}

export const i18n = $state({ locale: detectInitialLocale() });

/** Change la langue active (et la mémorise pour l'écran de déverrouillage). */
export function setLocale(code: string): void {
  const next = isSupported(code) ? code : REFERENCE_LOCALE;
  i18n.locale = next;
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    // ignoré
  }
}

function lookup(key: string): string | undefined {
  return catalogs[i18n.locale]?.[key] ?? catalogs[FALLBACK_LOCALE]?.[key] ?? catalogs[REFERENCE_LOCALE]?.[key];
}

export function interpolate(text: string, params?: Record<string, string | number>): string {
  if (!params) return text;
  return text.replace(/\{(\w+)\}/g, (m, name: string) => (name in params ? String(params[name]) : m));
}

/** Traduit une clé. Clé inconnue : la clé elle-même (visible, donc repérable). */
export function t(key: string, params?: Record<string, string | number>): string {
  return interpolate(lookup(key) ?? key, params);
}

/** Pluriel : `<clé>_one` / `<clé>_other` (catégories Intl.PluralRules), `{n}` fourni. */
export function tn(key: string, n: number, params?: Record<string, string | number>): string {
  let category = "other";
  try {
    category = new Intl.PluralRules(i18n.locale).select(n);
  } catch {
    // locale inconnue d'Intl : « other »
  }
  const text = lookup(`${key}_${category}`) ?? lookup(`${key}_other`) ?? key;
  return interpolate(text, { n, ...params });
}

/** Date locale courte (« 2026-11-20 22:59 » selon la langue). */
export function formatDateTime(epochSeconds: number | null): string {
  if (epochSeconds == null) return "";
  try {
    return new Intl.DateTimeFormat(i18n.locale, { dateStyle: "short", timeStyle: "short" }).format(
      new Date(epochSeconds * 1000),
    );
  } catch {
    return new Date(epochSeconds * 1000).toISOString();
  }
}
