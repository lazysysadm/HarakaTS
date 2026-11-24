// Aides pures de l'éditeur distant (M6) : langage, fins de ligne, état « modifié ».
import type { LanguageDescription } from "@codemirror/language";
import type { LineEnding } from "./api";

/** Taille maximale (octets) pour ouvrir un fichier dans l'éditeur intégré par double-clic. */
export const MAX_INLINE_EDIT = 5 * 1024 * 1024;

export type Eol = "lf" | "crlf";

/** Séparateur de lignes de l'éditeur pour une fin de ligne détectée. */
export function eolSeparator(eol: LineEnding | Eol): string {
  return eol === "crlf" ? "\r\n" : "\n";
}

/** Fin de ligne cible proposée par défaut (mixte/aucune → LF). */
export function targetEol(le: LineEnding): Eol {
  return le === "crlf" ? "crlf" : "lf";
}

/** Texte tel que l'éditeur le restitue après chargement sans modification. */
export function editorText(content: string, le: LineEnding): string {
  return le === "crlf" ? content : content.replace(/\r\n?/g, "\n");
}

/** Convertit toutes les fins de ligne de `text` vers `eol`. */
export function convertEol(text: string, eol: Eol): string {
  return text.replace(/\r\n|\r|\n/g, eolSeparator(eol));
}

/** Contenu à envoyer au serveur : le texte de l'éditeur converti vers la fin de ligne choisie. */
export function contentForSave(text: string, eol: Eol): string {
  return convertEol(text, eol);
}

/** Modifié : texte différent de la référence, ou fin de ligne changée. */
export function isDirty(
  current: string | null,
  baseline: string | null,
  eol: Eol,
  baseEol: Eol,
): boolean {
  if (current === null || baseline === null) return false;
  return current !== baseline || eol !== baseEol;
}

/** Chargement paresseux de la liste des langages (hors bundle principal). */
export async function languageFor(filename: string): Promise<LanguageDescription | null> {
  const [{ languages }, { LanguageDescription }] = await Promise.all([
    import("@codemirror/language-data"),
    import("@codemirror/language"),
  ]);
  return LanguageDescription.matchFilename(languages, filename);
}
