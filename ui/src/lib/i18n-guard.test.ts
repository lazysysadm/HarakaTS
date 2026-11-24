// Garde-fou : aucun texte visible en dur dans les composants Svelte.
// Tout texte (nœud texte, title, placeholder, aria-label…) doit passer par t()/tn().
import { parse } from "svelte/compiler";
import { describe, expect, it } from "vitest";

// Sources bruts de tous les composants (sans accès au système de fichiers).
const sources = import.meta.glob("../**/*.svelte", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/** Textes volontairement non traduits (noms de produit / techniques / universels). */
const ALLOWED = new Set([
  "OK",
  "HarakaTS",
  "HarakaTS —",
  "SSH / SFTP",
  "russh · russh-sftp (Rust)",
  "Argon2id · XChaCha20-Poly1305",
  "Ctrl+K",
  "user@host:port",
  "web-01.example.net",
  "server",
  "v",
]);

const ATTRS = new Set(["title", "placeholder", "aria-label", "label", "alt"]);

function hardcoded(source: string): string[] {
  const ast = parse(source, { modern: true });
  const found: string[] = [];
  const check = (raw: string) => {
    const text = raw.replace(/\s+/g, " ").trim();
    if (!text || ALLOWED.has(text)) return;
    if (/[A-Za-zÀ-ÿ]{3,}/.test(text)) found.push(text);
  };
  const walk = (n: unknown): void => {
    if (!n || typeof n !== "object") return;
    if (Array.isArray(n)) return n.forEach(walk);
    const node = n as Record<string, unknown> & { type?: string; name?: string; value?: unknown };
    if (node.type === "Text") check(String(node.data));
    if (node.type === "Attribute") {
      if (ATTRS.has(String(node.name)) && Array.isArray(node.value)) {
        for (const v of node.value as { type: string; data?: string }[]) if (v.type === "Text") check(v.data ?? "");
      }
      return; // les autres attributs (class, id, value…) ne sont pas du texte visible
    }
    for (const k of Object.keys(node)) if (!["type", "start", "end", "parent"].includes(k)) walk(node[k]);
  };
  walk(ast.fragment);
  return found;
}

describe("aucun texte visible en dur dans les composants", () => {
  for (const [file, source] of Object.entries(sources)) {
    it(file.replace(/^\.\.\//, ""), () => {
      expect(hardcoded(source)).toEqual([]);
    });
  }
});
