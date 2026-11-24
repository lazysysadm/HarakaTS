import { describe, expect, it } from "vitest";
import {
  contentForSave,
  convertEol,
  editorText,
  eolSeparator,
  isDirty,
  languageFor,
  targetEol,
} from "./edit";

describe("fins de ligne", () => {
  it("séparateur et cible", () => {
    expect(eolSeparator("crlf")).toBe("\r\n");
    expect(eolSeparator("lf")).toBe("\n");
    expect(eolSeparator("mixed")).toBe("\n");
    expect(targetEol("crlf")).toBe("crlf");
    expect(targetEol("mixed")).toBe("lf");
    expect(targetEol("none")).toBe("lf");
  });

  it("convertit dans les deux sens", () => {
    expect(convertEol("a\nb\r\nc\rd", "lf")).toBe("a\nb\nc\nd");
    expect(convertEol("a\nb\r\nc", "crlf")).toBe("a\r\nb\r\nc");
    expect(convertEol("", "crlf")).toBe("");
  });

  it("aller-retour exact pour du CRLF", () => {
    const raw = "un\r\ndeux\r\n";
    expect(editorText(raw, "crlf")).toBe(raw);
    expect(contentForSave(editorText(raw, "crlf"), "crlf")).toBe(raw);
  });

  it("aller-retour exact pour du LF", () => {
    const raw = "un\ndeux\n";
    expect(contentForSave(editorText(raw, "lf"), "lf")).toBe(raw);
  });

  it("normalise le mixte comme l'éditeur", () => {
    expect(editorText("a\r\nb\nc", "mixed")).toBe("a\nb\nc");
  });
});

describe("isDirty", () => {
  it("compare texte et fin de ligne", () => {
    expect(isDirty("a", "a", "lf", "lf")).toBe(false);
    expect(isDirty("ab", "a", "lf", "lf")).toBe(true);
    expect(isDirty("a", "a", "crlf", "lf")).toBe(true);
    expect(isDirty(null, "a", "lf", "lf")).toBe(false);
    expect(isDirty("a", null, "lf", "lf")).toBe(false);
  });
});

describe("languageFor", () => {
  it("trouve le langage d'après le nom", async () => {
    expect((await languageFor("main.ts"))?.name).toBe("TypeScript");
    expect((await languageFor("Dockerfile"))?.name).toBe("Dockerfile");
    expect((await languageFor("config.yaml"))?.name).toBe("YAML");
  });
  it("renvoie null si inconnu", async () => {
    expect(await languageFor("fichier.zzzunknown")).toBeNull();
  });
});
