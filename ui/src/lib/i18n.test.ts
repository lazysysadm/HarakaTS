import { describe, expect, it } from "vitest";
import { REFERENCE_LOCALE, availableLocales, catalogs, interpolate } from "./i18n.svelte";

const placeholders = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

describe("catalogues de langues", () => {
  const ref = catalogs[REFERENCE_LOCALE];

  it("la langue de référence existe et déclare son nom", () => {
    expect(ref).toBeDefined();
    expect(ref._name).toBeTruthy();
  });

  it("langues attendues présentes", () => {
    const codes = availableLocales().map((l) => l.code);
    for (const c of ["fr", "en", "it", "de", "es", "pt"]) expect(codes).toContain(c);
  });

  for (const [code, cat] of Object.entries(catalogs)) {
    describe(code, () => {
      it("a exactement les mêmes clés que la référence", () => {
        const missing = Object.keys(ref).filter((k) => !(k in cat));
        const extra = Object.keys(cat).filter((k) => !(k in ref));
        expect({ missing, extra }).toEqual({ missing: [], extra: [] });
      });

      it("n'a aucune valeur vide et déclare _name", () => {
        expect(cat._name).toBeTruthy();
        const empty = Object.entries(cat)
          .filter(([, v]) => typeof v !== "string" || v.trim() === "")
          .map(([k]) => k);
        expect(empty).toEqual([]);
      });

      it("conserve les paramètres {nom} de la référence", () => {
        const bad = Object.keys(ref)
          .filter((k) => k !== "_name" && k in cat)
          .filter((k) => JSON.stringify(placeholders(ref[k])) !== JSON.stringify(placeholders(cat[k])));
        expect(bad).toEqual([]);
      });
    });
  }
});

describe("interpolation", () => {
  it("remplace les paramètres connus et laisse les inconnus", () => {
    expect(interpolate("a {x} b {y}", { x: 1 })).toBe("a 1 b {y}");
  });
});
