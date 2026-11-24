import { describe, expect, it } from "vitest";
import { RULES, matchRule, translateError } from "./errors";

describe("errors rules", () => {
  it("has broad coverage with valid unique keys", () => {
    expect(RULES.length).toBeGreaterThanOrEqual(50);
    const keys = RULES.map((r) => r.key);
    for (const k of keys) {
      expect(k.length).toBeGreaterThan(0);
      expect(k.startsWith("be.")).toBe(true);
    }
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("every regex rule declares one param per capture group", () => {
    for (const r of RULES) {
      if (typeof r.pattern === "string") continue;
      const groups = new RegExp(r.pattern.source + "|").exec("")!.length - 1;
      expect(r.params?.length ?? 0).toBe(groups);
    }
  });

  it.each([
    ["coffre verrouillé", "be.vault.locked"],
    ["mot de passe ou passphrase incorrect", "be.auth.wrongPassword"],
    ["données altérées ou corrompues (échec d'authentification)", "be.data.corrupt"],
    ["Les deux saisies du mot de passe maître diffèrent.", "be.master.mismatch"],
    ["Export annulé.", "be.export.cancelled"],
    ["connexion introuvable", "be.connection.notFound"],
    ["c'est une clé PUBLIQUE — importez la clé privée (fichier sans .pub)", "be.key.isPublic"],
    ["clé d'hôte changée — connexion bloquée (vérification requise)", "be.ssh.hostKeyChanged"],
    ["session fermée", "be.ssh.sessionClosed"],
    ["fermée par l'utilisateur", "be.close.byUser"],
  ])("exact message %s", (msg, key) => {
    expect(matchRule(msg)).toEqual({ key, params: {}, nested: [] });
  });

  it("extracts params and keeps OS tails untouched", () => {
    expect(matchRule("presse-papier indisponible : X11 error (os error 13)")).toEqual({
      key: "be.clipboard.unavailable",
      params: { detail: "X11 error (os error 13)" },
      nested: [],
    });
    expect(matchRule("taille inattendue pour /a/b : 10 octets reçus, 12 attendus")).toEqual({
      key: "be.sftp.sizeMismatchRecv",
      params: { path: "/a/b", got: "10", expected: "12" },
      nested: [],
    });
    expect(matchRule("déconnexion serveur : Disconnect { reason }")?.params).toEqual({
      info: "Disconnect { reason }",
    });
    expect(matchRule("port invalide : abc")?.params).toEqual({ port: "abc" });
  });

  it("flags nested wrappers", () => {
    const m = matchRule("paramètres invalides : hôte vide");
    expect(m?.key).toBe("be.core.invalidParams");
    expect(m?.nested).toEqual(["detail"]);
    expect(matchRule("hôte vide")?.key).toBe("be.param.hostEmpty");
    expect(matchRule("élément introuvable : dossier")?.params).toEqual({ what: "dossier" });
    expect(matchRule("connexion impossible : chaîne vide")?.nested).toEqual(["detail"]);
  });

  it("passes unknown messages through unchanged", () => {
    expect(matchRule("quelque chose d'inattendu")).toBeNull();
    expect(translateError("Permission denied (os error 13)")).toBe(
      "Permission denied (os error 13)",
    );
  });
});
