import { describe, expect, it } from "vitest";
import { matchRule } from "./errors";
import { RULES_TUNNELS } from "./errors.tunnels";

describe("errors tunnels rules", () => {
  it("has unique be.tunnel.* keys", () => {
    const keys = RULES_TUNNELS.map((r) => r.key);
    expect(new Set(keys).size).toBe(keys.length);
    for (const k of keys) expect(k.startsWith("be.tunnel.")).toBe(true);
  });

  it.each([
    ["tunnel : hôte et port de destination requis", "be.tunnel.destRequired", {}],
    ["tunnel : tunnel introuvable", "be.tunnel.notFound", {}],
    ["tunnel : adresse d'écoute invalide « x y »", "be.tunnel.badBind", { addr: "x y" }],
    ["tunnel : type de tunnel inconnu « zz »", "be.tunnel.unknownKind", { kind: "zz" }],
    [
      "tunnel : impossible d'écouter sur 127.0.0.1:8080 : port déjà utilisé",
      "be.tunnel.inUse",
      { addr: "127.0.0.1:8080" },
    ],
    [
      "tunnel : impossible d'écouter sur 0.0.0.0:80 : permission refusée",
      "be.tunnel.denied",
      { addr: "0.0.0.0:80" },
    ],
    [
      "tunnel : impossible d'écouter sur 127.0.0.1:1 : boum (os error 5)",
      "be.tunnel.listenFailed",
      { addr: "127.0.0.1:1", detail: "boum (os error 5)" },
    ],
    [
      "tunnel : le serveur a refusé la redirection distante (port occupé ou redirection interdite)",
      "be.tunnel.remoteRefused",
      {},
    ],
    ["tunnel : autre chose", "be.tunnel.generic", { detail: "autre chose" }],
    ["tunnel enregistré introuvable", "be.tunnel.savedNotFound", {}],
  ])("matches %s", (msg, key, params) => {
    expect(matchRule(msg)).toEqual({ key, params, nested: [] });
  });
});
