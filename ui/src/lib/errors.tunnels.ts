// Règles de traduction des erreurs backend — domaine « tunnels » (M7).
// Même format que `errors.ts` ; ajouter ici uniquement les messages de ce domaine.
import type { Rule } from "./errors";

export const RULES_TUNNELS: Rule[] = [
  { key: "be.tunnel.badBind", pattern: /^tunnel : adresse d'écoute invalide « (.*) »$/s, params: ["addr"] },
  { key: "be.tunnel.destRequired", pattern: "tunnel : hôte et port de destination requis" },
  { key: "be.tunnel.idEmpty", pattern: "tunnel : identifiant de tunnel vide" },
  { key: "be.tunnel.unknownKind", pattern: /^tunnel : type de tunnel inconnu « (.*) »$/s, params: ["kind"] },
  { key: "be.tunnel.notFound", pattern: "tunnel : tunnel introuvable" },
  { key: "be.tunnel.inUse", pattern: /^tunnel : impossible d'écouter sur (.+) : port déjà utilisé$/s, params: ["addr"] },
  { key: "be.tunnel.denied", pattern: /^tunnel : impossible d'écouter sur (.+) : permission refusée$/s, params: ["addr"] },
  {
    key: "be.tunnel.addrUnavailable",
    pattern: /^tunnel : impossible d'écouter sur (.+) : adresse indisponible$/s,
    params: ["addr"],
  },
  {
    key: "be.tunnel.listenFailed",
    pattern: /^tunnel : impossible d'écouter sur (\S+) : (.+)$/s,
    params: ["addr", "detail"],
  },
  {
    key: "be.tunnel.remoteRefused",
    pattern: "tunnel : le serveur a refusé la redirection distante (port occupé ou redirection interdite)",
  },
  {
    key: "be.tunnel.remoteTimeout",
    pattern: "tunnel : délai dépassé : le serveur n'a pas répondu à la redirection distante",
  },
  { key: "be.tunnel.savedNotFound", pattern: "tunnel enregistré introuvable" },
  { key: "be.tunnel.specRequired", pattern: "tunnelId ou spec requis" },
  { key: "be.tunnel.generic", pattern: /^tunnel : (.+)$/s, params: ["detail"] },
];
