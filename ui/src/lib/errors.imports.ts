// Règles de traduction des erreurs backend — domaine « imports » (M7).
// Même format que `errors.ts` ; ajouter ici uniquement les messages de ce domaine.
import type { Rule } from "./errors";

export const RULES_IMPORTS: Rule[] = [
  { key: "be.import.noRemminaFiles", pattern: "aucun fichier .remmina trouvé dans ce dossier" },
  { key: "be.import.tooManyFiles", pattern: "trop de fichiers sélectionnés (500 maximum)" },
  { key: "be.import.unknownSource", pattern: "source d'import inconnue" },
  { key: "be.import.internal", pattern: "état interne indisponible" },
  { key: "be.import.fileUnreadable", pattern: /^fichier illisible : (.+)$/s, params: ["detail"] },
];
