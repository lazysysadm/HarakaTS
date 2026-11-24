// Règles de traduction des erreurs backend — domaine « multiexec » (M7).
// Même format que `errors.ts` ; ajouter ici uniquement les messages de ce domaine.
import type { Rule } from "./errors";

export const RULES_MULTIEXEC: Rule[] = [
  { key: "be.multiexec.minSessions", pattern: "multi-exec : au moins 2 sessions sont requises" },
  { key: "be.multiexec.inactive", pattern: "multi-exec inactif" },
  { key: "be.macro.confirmRequired", pattern: "confirmation requise pour cette macro" },
  { key: "be.macro.sessionNotConnected", pattern: "session non connectée" },
];
