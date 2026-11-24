// Thème CodeMirror suivant les variables CSS de l'application (clair/sombre),
// traductions des panneaux CodeMirror et chargement paresseux des langages.
import { HighlightStyle, syntaxHighlighting, type LanguageSupport } from "@codemirror/language";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { tags as g } from "@lezer/highlight";
import { languageFor } from "./edit";
import { t } from "./i18n.svelte";

/** Sombre ou clair, d'après la luminance de `--bg` (suit les thèmes de l'application). */
export function isDarkTheme(): boolean {
  const raw = getComputedStyle(document.documentElement).getPropertyValue("--bg").trim();
  const m = /^#([0-9a-f]{6})$/i.exec(raw);
  if (!m) return window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? true;
  const n = parseInt(m[1], 16);
  const lum = (0.299 * ((n >> 16) & 255) + 0.587 * ((n >> 8) & 255) + 0.114 * (n & 255)) / 255;
  return lum < 0.5;
}

const dark = HighlightStyle.define([
  { tag: [g.keyword, g.operatorKeyword, g.modifier, g.controlKeyword], color: "#cba6f7" },
  { tag: [g.string, g.special(g.string), g.regexp], color: "#a6e3a1" },
  { tag: [g.number, g.bool, g.null, g.atom], color: "#fab387" },
  { tag: [g.comment, g.lineComment, g.blockComment], color: "#7f849c", fontStyle: "italic" },
  { tag: [g.function(g.variableName), g.function(g.propertyName)], color: "#89b4fa" },
  { tag: [g.typeName, g.className, g.namespace], color: "#f9e2af" },
  { tag: [g.propertyName, g.attributeName], color: "#94e2d5" },
  { tag: [g.tagName, g.heading], color: "#f38ba8" },
  { tag: [g.operator, g.punctuation, g.meta], color: "#9399b2" },
  { tag: g.link, color: "#89b4fa", textDecoration: "underline" },
  { tag: g.invalid, color: "#f38ba8" },
]);

const light = HighlightStyle.define([
  { tag: [g.keyword, g.operatorKeyword, g.modifier, g.controlKeyword], color: "#8839ef" },
  { tag: [g.string, g.special(g.string), g.regexp], color: "#40a02b" },
  { tag: [g.number, g.bool, g.null, g.atom], color: "#c25b00" },
  { tag: [g.comment, g.lineComment, g.blockComment], color: "#8c8fa1", fontStyle: "italic" },
  { tag: [g.function(g.variableName), g.function(g.propertyName)], color: "#1e66f5" },
  { tag: [g.typeName, g.className, g.namespace], color: "#a16a00" },
  { tag: [g.propertyName, g.attributeName], color: "#179299" },
  { tag: [g.tagName, g.heading], color: "#d20f39" },
  { tag: [g.operator, g.punctuation, g.meta], color: "#6c6f85" },
  { tag: g.link, color: "#1e66f5", textDecoration: "underline" },
  { tag: g.invalid, color: "#d20f39" },
]);

/** Thème de base + coloration syntaxique, en tenant compte du mode sombre/clair. */
export function appTheme(isDark: boolean): Extension {
  return [
    EditorView.theme(
      {
        "&": { color: "var(--text)", backgroundColor: "var(--bg)", height: "100%", fontSize: "13px" },
        "&.cm-focused": { outline: "none" },
        ".cm-scroller": { fontFamily: "ui-monospace, 'JetBrains Mono', 'DejaVu Sans Mono', monospace", lineHeight: "1.45" },
        ".cm-content": { caretColor: "var(--accent)" },
        ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)" },
        "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
          background: "color-mix(in srgb, var(--accent) 30%, transparent)",
        },
        ".cm-gutters": { backgroundColor: "var(--panel)", color: "var(--muted)", border: "none", borderRight: "1px solid var(--border)" },
        ".cm-activeLine": { backgroundColor: "color-mix(in srgb, var(--accent) 8%, transparent)" },
        ".cm-activeLineGutter": { backgroundColor: "color-mix(in srgb, var(--accent) 14%, transparent)", color: "var(--text)" },
        ".cm-matchingBracket, &.cm-focused .cm-matchingBracket": { backgroundColor: "color-mix(in srgb, var(--accent) 28%, transparent)", outline: "1px solid var(--accent)" },
        ".cm-searchMatch": { backgroundColor: "color-mix(in srgb, var(--accent) 25%, transparent)", outline: "1px solid var(--accent)" },
        ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "color-mix(in srgb, var(--accent) 50%, transparent)" },
        ".cm-selectionMatch": { backgroundColor: "color-mix(in srgb, var(--accent) 18%, transparent)" },
        ".cm-panels": { backgroundColor: "var(--panel)", color: "var(--text)", borderColor: "var(--border)" },
        ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--border)" },
        ".cm-panel.cm-search": { padding: "4px 8px", display: "flex", flexWrap: "wrap", alignItems: "center", gap: "4px" },
        ".cm-panel.cm-search input, .cm-panel.cm-search button": { margin: "0", padding: "2px 6px", fontSize: "12px", width: "auto" },
        ".cm-panel.cm-search label": { display: "inline-flex", alignItems: "center", gap: "3px", margin: "0 4px", fontSize: "12px" },
        ".cm-panel.cm-search [name=close]": { position: "static", marginLeft: "auto", background: "transparent", color: "var(--muted)", border: "none" },
        ".cm-tooltip": { backgroundColor: "var(--panel)", color: "var(--text)", border: "1px solid var(--border)" },
        ".cm-foldPlaceholder": { backgroundColor: "var(--bg)", color: "var(--muted)", border: "1px solid var(--border)" },
        // Vue de comparaison (@codemirror/merge)
        "&.cm-merge-a .cm-changedLine, .cm-deletedChunk": { backgroundColor: "color-mix(in srgb, var(--danger) 12%, transparent)" },
        "&.cm-merge-b .cm-changedLine, .cm-inlineChangedLine": { backgroundColor: "color-mix(in srgb, var(--ok) 12%, transparent)" },
        ".cm-collapsedLines": { backgroundColor: "var(--panel)", color: "var(--muted)" },
      },
      { dark: isDark },
    ),
    syntaxHighlighting(isDark ? dark : light),
  ];
}

/** Textes des panneaux CodeMirror (recherche, aller à la ligne, comparaison). */
export function cmPhrases(): Extension {
  return EditorState.phrases.of({
    Find: t("edit.cm.find"),
    Replace: t("edit.cm.replace"),
    next: t("edit.cm.next"),
    previous: t("edit.cm.previous"),
    all: t("edit.cm.all"),
    "match case": t("edit.cm.matchCase"),
    regexp: t("edit.cm.regexp"),
    "by word": t("edit.cm.byWord"),
    replace: t("edit.cm.replaceOne"),
    "replace all": t("edit.cm.replaceAll"),
    close: t("edit.cm.close"),
    "Go to line": t("edit.cm.gotoLine"),
    go: t("edit.cm.go"),
    "Revert this chunk": t("edit.cm.revertChunk"),
    "Controls for merge view": t("edit.cm.mergeControls"),
    "Expand": t("edit.cm.expand"),
    "$ unchanged lines": t("edit.cm.unchangedLines"),
  });
}

/** Charge le support de langage d'un nom de fichier (nul : texte brut ou échec). */
export async function loadLanguage(
  filename: string,
): Promise<{ name: string; support: LanguageSupport } | null> {
  try {
    const desc = await languageFor(filename);
    if (!desc) return null;
    return { name: desc.name, support: await desc.load() };
  } catch {
    return null;
  }
}
