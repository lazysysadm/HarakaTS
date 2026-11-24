// Traduction des messages d'erreur français renvoyés par le backend Rust.
// Table de règles : chaîne exacte ou RegExp (groupes de capture → params).
// Les queues variables (erreurs OS / bibliothèques) restent intactes.
import { t } from "./i18n.svelte";
import { RULES_IMPORTS } from "./errors.imports";
import { RULES_MULTIEXEC } from "./errors.multiexec";
import { RULES_TUNNELS } from "./errors.tunnels";

export interface Rule {
  key: string;
  /** Chaîne exacte ou expression régulière ancrée. */
  pattern: string | RegExp;
  /** Noms des groupes de capture, dans l'ordre. */
  params?: string[];
  /** Params à traduire récursivement (message interne). */
  nested?: string[];
}

const CORE_RULES: Rule[] = [
  { key: "be.vault.locked", pattern: "coffre verrouillé" },
  { key: "be.vault.exists", pattern: "un coffre existe déjà dans ce répertoire de données" },
  { key: "be.auth.wrongPassword", pattern: "mot de passe ou passphrase incorrect" },
  { key: "be.data.corrupt", pattern: "données altérées ou corrompues (échec d'authentification)" },
  { key: "be.crypto.internal", pattern: "erreur cryptographique interne" },
  { key: "be.io.error", pattern: "erreur d'entrée/sortie" },
  { key: "be.serialize.error", pattern: "erreur de sérialisation" },
  { key: "be.master.mismatch", pattern: "Les deux saisies du mot de passe maître diffèrent." },
  { key: "be.master.tooShort", pattern: "Mot de passe maître trop court (8 caractères minimum)." },
  { key: "be.master.required", pattern: "mot de passe maître requis" },
  { key: "be.newMaster.mismatch", pattern: "Les deux saisies du nouveau mot de passe diffèrent." },
  { key: "be.newMaster.tooShort", pattern: "Nouveau mot de passe trop court (8 caractères minimum)." },
  { key: "be.backup.lockFirst", pattern: "verrouillez le coffre avant de restaurer une sauvegarde" },
  { key: "be.settings.patchInvalid", pattern: "patch de réglages invalide" },
  { key: "be.settings.invalid", pattern: /^réglages invalides : (.+)$/s, params: ["detail"] },
  { key: "be.uiState.patchInvalid", pattern: "patch ui_state invalide" },
  { key: "be.export.mismatch", pattern: "Les deux saisies de la passphrase d'export diffèrent." },
  { key: "be.export.tooShort", pattern: "Passphrase d'export trop courte (8 caractères minimum)." },
  { key: "be.export.cancelled", pattern: "Export annulé." },
  { key: "be.import.cancelled", pattern: "Import annulé." },
  { key: "be.import.previewExpired", pattern: "aperçu expiré — relancez l'import" },
  { key: "be.connection.notFound", pattern: "connexion introuvable" },
  { key: "be.connection.notFoundAfterCreate", pattern: "connexion introuvable après création" },
  { key: "be.key.noFile", pattern: "aucun fichier sélectionné" },
  { key: "be.key.tooBig", pattern: "fichier trop volumineux (max 64 Ko) — ce n'est pas une clé privée" },
  { key: "be.key.binary", pattern: "fichier binaire : ce n'est pas une clé privée" },
  { key: "be.key.isPublic", pattern: "c'est une clé PUBLIQUE — importez la clé privée (fichier sans .pub)" },
  { key: "be.key.notPrivate", pattern: "ce fichier n'est pas une clé privée (en-tête -----BEGIN … PRIVATE KEY----- absent)" },
  { key: "be.key.passphraseRequired", pattern: "passphrase requise : cette clé est chiffrée" },
  { key: "be.key.unreadable", pattern: /^clé illisible : (.+)$/s, params: ["detail"] },
  { key: "be.key.invalidOrWrongPassphrase", pattern: "clé privée invalide ou passphrase incorrecte" },
  { key: "be.clipboard.unavailable", pattern: /^presse-papier indisponible : (.+)$/s, params: ["detail"] },
  { key: "be.clipboard.copyFailed", pattern: /^copie impossible : (.+)$/s, params: ["detail"] },
  { key: "be.clipboard.readFailed", pattern: /^lecture impossible : (.+)$/s, params: ["detail"] },
  { key: "be.macro.nameEmpty", pattern: "nom de macro vide" },
  { key: "be.macro.notFound", pattern: "macro introuvable" },
  { key: "be.macro.contentRequired", pattern: "contenu de macro requis à la création" },
  { key: "be.session.userRequired", pattern: "utilisateur requis (connexion, dossier ou credential)" },
  { key: "be.session.vteClosed", pattern: "fenêtre VTE fermée avant l'ouverture de la session" },
  { key: "be.session.vteLinuxOnly", pattern: "le moteur VTE natif requiert Linux" },
  { key: "be.session.notFoundShort", pattern: "session introuvable" },
  { key: "be.session.nativeNotFound", pattern: "session native introuvable" },
  { key: "be.env.homeNotFound", pattern: "HOME introuvable" },
  { key: "be.core.badFormat", pattern: /^format de fichier invalide : (.+)$/s, params: ["detail"], nested: ["detail"] },
  { key: "be.core.unsupportedVersion", pattern: /^version de format non supportée : (.+) \(mettez HarakaTS à jour\)$/s, params: ["version"] },
  { key: "be.core.unsupportedSchema", pattern: /^schéma interne non supporté : (.+)$/s, params: ["version"] },
  { key: "be.core.notFound", pattern: /^élément introuvable : (.+)$/s, params: ["what"], nested: ["what"] },
  { key: "be.core.invalidRef", pattern: /^référence invalide : (.+)$/s, params: ["detail"], nested: ["detail"] },
  { key: "be.core.invalidParams", pattern: /^paramètres invalides : (.+)$/s, params: ["detail"], nested: ["detail"] },
  { key: "be.ref.orphanRecent", pattern: "récente orpheline" },
  { key: "be.noun.backup", pattern: "sauvegarde" },
  { key: "be.noun.vault", pattern: "coffre" },
  { key: "be.noun.secretField", pattern: "champ secret" },
  { key: "be.noun.keyringCache", pattern: "cache keyring" },
  { key: "be.noun.knownHostsSource", pattern: "known_hosts source" },
  { key: "be.noun.macro", pattern: "macro" },
  { key: "be.noun.folder", pattern: "dossier" },
  { key: "be.noun.connection", pattern: "connexion" },
  { key: "be.noun.credential", pattern: "credential" },
  { key: "be.format.tooBig", pattern: "fichier trop volumineux" },
  { key: "be.format.magicTruncated", pattern: "magic tronqué" },
  { key: "be.format.magicUnknown", pattern: "magic inconnu" },
  { key: "be.format.versionTruncated", pattern: "version tronquée" },
  { key: "be.format.headerTooBig", pattern: "en-tête trop volumineux" },
  { key: "be.format.kdfTooBig", pattern: "bloc KDF trop volumineux" },
  { key: "be.format.dekNonceTruncated", pattern: "dek_nonce tronqué" },
  { key: "be.format.payloadNonceTruncated", pattern: "payload_nonce tronqué" },
  { key: "be.format.payloadTruncated", pattern: "payload tronqué" },
  { key: "be.format.fileTruncated", pattern: "fichier tronqué" },
  { key: "be.format.lengthTruncated", pattern: "longueur tronquée" },
  { key: "be.format.exportMagic", pattern: "magic d'export attendu" },
  { key: "be.format.nonceSize", pattern: "nonce de taille invalide" },
  { key: "be.format.pubkeyBase64", pattern: "clé publique base64 invalide" },
  { key: "be.format.vaultMagic", pattern: "magic de coffre attendu" },
  { key: "be.format.kdfUnsupported", pattern: "bloc KDF non supporté" },
  { key: "be.log.invalidName", pattern: "nom de journal invalide" },
  { key: "be.format.logTruncated", pattern: "journal tronqué" },
  { key: "be.format.logMagic", pattern: "magic de journal inconnu" },
  { key: "be.format.logRecordTooBig", pattern: "bloc de journal trop volumineux" },
  { key: "be.format.backupCorrupt", pattern: "sauvegarde corrompue" },
  { key: "be.format.writeUnverified", pattern: "écriture du coffre non vérifiée" },
  { key: "be.format.base64", pattern: "base64 invalide" },
  { key: "be.param.folderIdRequired", pattern: "folder_id requis" },
  { key: "be.param.jumpHostEmpty", pattern: "hôte de jump vide" },
  { key: "be.param.maxDepth", pattern: "profondeur maximale atteinte" },
  { key: "be.param.folderNameEmpty", pattern: "nom de dossier vide" },
  { key: "be.param.connNameEmpty", pattern: "nom de connexion vide" },
  { key: "be.param.hostEmpty", pattern: "hôte vide" },
  { key: "be.param.credNameEmpty", pattern: "nom de credential vide" },
  { key: "be.param.inputEmpty", pattern: "saisie vide" },
  { key: "be.param.userEmptyBeforeAt", pattern: "utilisateur vide avant @" },
  { key: "be.param.ipv6Bracket", pattern: "IPv6 : crochet fermant manquant" },
  { key: "be.param.ipv6Empty", pattern: "adresse IPv6 vide" },
  { key: "be.param.ipv6Format", pattern: "format attendu : [adresse]:port" },
  { key: "be.param.port0", pattern: "port 0 invalide" },
  { key: "be.param.argonBounds", pattern: "paramètres Argon2id hors bornes" },
  { key: "be.param.argonDerive", pattern: "dérivation Argon2id impossible" },
  { key: "be.param.unknownScope", pattern: /^portée inconnue : (.+)$/s, params: ["scope"] },
  { key: "be.param.unknownStrategy", pattern: /^stratégie inconnue : (.+)$/s, params: ["strategy"] },
  { key: "be.param.invalidPort", pattern: /^port invalide : (.+)$/s, params: ["port"] },
  { key: "be.ssh.authFailed", pattern: "échec d'authentification (toutes les méthodes disponibles ont été tentées)" },
  { key: "be.ssh.connectFailed", pattern: /^connexion impossible : (.+)$/s, params: ["detail"], nested: ["detail"] },
  { key: "be.ssh.emptyChain", pattern: "chaîne vide" },
  { key: "be.ssh.hostKeyRejected", pattern: "clé d'hôte refusée par l'utilisateur" },
  { key: "be.ssh.hostKeyChanged", pattern: "clé d'hôte changée — connexion bloquée (vérification requise)" },
  { key: "be.ssh.sessionNotFound", pattern: /^session introuvable : (.+)$/s, params: ["id"] },
  { key: "be.ssh.sessionClosed", pattern: "session fermée" },
  { key: "be.ssh.promptTimeout", pattern: "délai d'attente dépassé (prompt utilisateur)" },
  { key: "be.ssh.error", pattern: /^erreur SSH : (.+)$/s, params: ["detail"] },
  { key: "be.edit.notRegular", pattern: /^SFTP : n'est pas un fichier régulier : (.+)$/s, params: ["path"] },
  { key: "be.edit.tooBig", pattern: /^SFTP : fichier trop volumineux pour l'édition \((\d+) octets, max (\d+)\)$/, params: ["size", "max"] },
  { key: "be.edit.tooBigShort", pattern: "SFTP : fichier trop volumineux pour l'édition" },
  { key: "be.edit.remoteTooBig", pattern: "SFTP : fichier distant trop volumineux" },
  { key: "be.edit.emptyCommand", pattern: "SFTP : commande d'éditeur vide" },
  { key: "be.edit.unclosedQuote", pattern: "SFTP : guillemet non fermé dans la commande de l'éditeur" },
  { key: "be.edit.watchFailed", pattern: /^SFTP : surveillance impossible : (.+)$/s, params: ["detail"] },
  { key: "be.edit.launchFailed", pattern: /^SFTP : éditeur externe impossible à lancer \((.+?)\) : (.+)$/s, params: ["program", "detail"] },
  { key: "be.sftp.error", pattern: /^SFTP : (.+)$/s, params: ["detail"] },
  { key: "be.ssh.agentUnavailable", pattern: "agent SSH indisponible" },
  { key: "be.sftp.sizeMismatchRecv", pattern: /^taille inattendue pour (.+) : (\d+) octets reçus, (\d+) attendus$/s, params: ["path", "got", "expected"] },
  { key: "be.sftp.sizeMismatchSent", pattern: /^taille inattendue pour (.+) : (\d+) octets écrits, (\d+) attendus$/s, params: ["path", "got", "expected"] },
  { key: "be.sftp.badLocalPath", pattern: /^chemin local invalide : (.+)$/s, params: ["path"] },
  { key: "be.close.byUser", pattern: "fermée par l'utilisateur" },
  { key: "be.close.serverDisconnect", pattern: /^déconnexion serveur : (.+)$/s, params: ["info"] },
  { key: "be.close.error", pattern: /^erreur : (.+)$/s, params: ["detail"] },
];

/** Règles du cœur puis des fonctionnalités M7 (un fichier par domaine, sans conflit d'édition). */
export const RULES: Rule[] = [
  ...CORE_RULES,
  ...RULES_TUNNELS,
  ...RULES_IMPORTS,
  ...RULES_MULTIEXEC,
];

export interface Match {
  key: string;
  params: Record<string, string>;
  nested: string[];
}

/** Première règle correspondant au message, ou null. */
export function matchRule(message: string): Match | null {
  for (const r of RULES) {
    if (typeof r.pattern === "string") {
      if (message === r.pattern) return { key: r.key, params: {}, nested: [] };
      continue;
    }
    const m = r.pattern.exec(message);
    if (!m) continue;
    const params: Record<string, string> = {};
    (r.params ?? []).forEach((name, i) => {
      params[name] = m[i + 1] ?? "";
    });
    return { key: r.key, params, nested: r.nested ?? [] };
  }
  return null;
}

/** Traduit un message backend ; inconnu → renvoyé tel quel. */
export function translateError(message: string): string {
  const m = matchRule(message);
  if (!m) return message;
  const params: Record<string, string> = { ...m.params };
  for (const n of m.nested) params[n] = translateError(params[n]);
  return t(m.key, params);
}
