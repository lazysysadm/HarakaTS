// Garde-fou d'accessibilité : contraste WCAG des couleurs de chaque thème (variables de app.css).
import { describe, expect, it } from "vitest";

// Lecture directe du fichier (vitest ne fournit pas le contenu des CSS via `?raw`) : module Node
// importé dynamiquement pour ne pas imposer les types Node au reste de l'interface.
const nodeFs = "node:fs";
const { readFileSync } = (await import(/* @vite-ignore */ nodeFs)) as {
  readFileSync: (path: URL, encoding: string) => string;
};
const css = readFileSync(new URL("../app.css", import.meta.url), "utf8");

function vars(block: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const m of block.matchAll(/(--[\w-]+):\s*([^;]+);/g)) out[m[1]] = m[2].trim();
  return out;
}

function blockAfter(marker: string): string {
  const start = css.indexOf(marker);
  if (start < 0) throw new Error(`bloc introuvable : ${marker}`);
  return css.slice(css.indexOf("{", start) + 1, css.indexOf("}", start));
}

const base = vars(blockAfter(":root {"));
const themes: Record<string, Record<string, string>> = {
  sombre: base,
  gris: { ...base, ...vars(blockAfter(':root[data-theme="gris"]')) },
  bleu: { ...base, ...vars(blockAfter(':root[data-theme="bleu"]')) },
  clair: { ...base, ...vars(blockAfter(':root[data-theme="clair"]')) },
  "système clair": { ...base, ...vars(blockAfter('  :root[data-theme="system"] {')) },
};

function luminance(hex: string): number {
  const h = hex.replace("#", "");
  const [r, g, b] = [0, 2, 4].map((i) => {
    const c = parseInt(h.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function ratio(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

// [premier plan, arrière-plan, minimum] : 4,5 = texte (AA), 3 = élément graphique.
const PAIRS: Array<[string, string, number]> = [
  ["--text", "--bg", 4.5],
  ["--text", "--panel", 4.5],
  ["--muted", "--bg", 4.5],
  ["--muted", "--panel", 4.5],
  ["--accent", "--panel", 4.5],
  ["--accent", "--bg", 4.5],
  ["--btn-text", "--accent", 4.5],
  ["--text", "--gray-btn", 4.5],
  ["--danger", "--panel", 4.5],
  ["--ok", "--panel", 4.5],
  ["--ok", "--bg", 4.5],
  ["--on-alert", "--alert", 4.5],
  ["--ok-strong", "--panel", 3],
  ["--bad", "--panel", 3],
  ["--warn", "--panel", 3],
  ["--fav", "--panel", 3],
  ["--neutral", "--panel", 3],
];

describe("contraste des thèmes (WCAG AA)", () => {
  for (const [name, t] of Object.entries(themes)) {
    for (const [fg, bg, min] of PAIRS) {
      it(`${name} : ${fg} sur ${bg} ≥ ${min}`, () => {
        expect(t[fg], `${fg} défini`).toMatch(/^#[0-9a-f]{6}$/i);
        expect(t[bg], `${bg} défini`).toMatch(/^#[0-9a-f]{6}$/i);
        expect(ratio(t[fg], t[bg])).toBeGreaterThanOrEqual(min);
      });
    }
  }
});
