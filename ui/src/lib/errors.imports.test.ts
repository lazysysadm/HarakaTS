import { describe, expect, it } from "vitest";
import { matchRule } from "./errors";
import { RULES_IMPORTS } from "./errors.imports";

describe("errors imports rules", () => {
  it("keys are namespaced and unique", () => {
    const keys = RULES_IMPORTS.map((r) => r.key);
    expect(keys.every((k) => k.startsWith("be.import."))).toBe(true);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it.each([
    ["aucun fichier .remmina trouvé dans ce dossier", "be.import.noRemminaFiles"],
    ["trop de fichiers sélectionnés (500 maximum)", "be.import.tooManyFiles"],
    ["source d'import inconnue", "be.import.unknownSource"],
    ["état interne indisponible", "be.import.internal"],
  ])("%s -> %s", (msg, key) => {
    expect(matchRule(msg)?.key).toBe(key);
  });

  it("captures the OS detail of unreadable files", () => {
    const m = matchRule("fichier illisible : Permission denied (os error 13)");
    expect(m?.key).toBe("be.import.fileUnreadable");
    expect(m?.params).toEqual({ detail: "Permission denied (os error 13)" });
  });
});
