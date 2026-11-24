import { describe, expect, it } from "vitest";
import { matchRule } from "./errors";
import { RULES_MULTIEXEC } from "./errors.multiexec";

describe("errors multiexec rules", () => {
  it("has unique be.* keys", () => {
    const keys = RULES_MULTIEXEC.map((r) => r.key);
    expect(new Set(keys).size).toBe(keys.length);
    for (const k of keys) expect(k.startsWith("be.")).toBe(true);
  });

  it.each([
    ["multi-exec : au moins 2 sessions sont requises", "be.multiexec.minSessions"],
    ["multi-exec inactif", "be.multiexec.inactive"],
    ["confirmation requise pour cette macro", "be.macro.confirmRequired"],
    ["session non connectée", "be.macro.sessionNotConnected"],
    ["macro introuvable", "be.macro.notFound"],
  ])("matches %s", (msg, key) => {
    expect(matchRule(msg)).toEqual({ key, params: {}, nested: [] });
  });
});
