import { beforeAll, describe, expect, it } from "vitest";
import { catalogs, setLocale } from "./i18n.svelte";
import {
  breadcrumbs,
  formatEta,
  formatRate,
  formatSize,
  joinPath,
  modeOctal,
  modeString,
  normalizePath,
  parentPath,
  parseOctalMode,
  progressPercent,
  resolvePath,
  sortEntries,
  type FileEntry,
} from "./sftp";

// Clés d'unités (également présentes dans fr.json après fusion).
beforeAll(() => {
  Object.assign(catalogs.fr, {
    "unit.b": "o",
    "unit.kib": "Kio",
    "unit.mib": "Mio",
    "unit.gib": "Gio",
    "unit.tib": "Tio",
    "unit.sec": "s",
    "unit.min": "min",
    "unit.hour": "h",
    "unit.perSec": "{size}/s",
  });
  setLocale("fr");
});

const e = (name: string, kind: FileEntry["kind"], size = 0, mtime: number | null = null): FileEntry => ({
  name,
  path: `/x/${name}`,
  kind,
  size,
  mtime,
  mode: 0o644,
});

describe("chemins", () => {
  it("join / parent / breadcrumbs", () => {
    expect(joinPath("/a", "b")).toBe("/a/b");
    expect(joinPath("/", "b")).toBe("/b");
    expect(parentPath("/a/b/")).toBe("/a");
    expect(parentPath("/a")).toBe("/");
    expect(parentPath("/")).toBe("/");
    expect(breadcrumbs("/usr/local").map((b) => b.path)).toEqual(["/", "/usr", "/usr/local"]);
  });
});

describe("formats", () => {
  it("taille, ETA, pourcentage", () => {
    expect(formatSize(512)).toBe("512 o");
    expect(formatSize(1536)).toBe("1.5 Kio");
    expect(formatSize(5 * 1024 * 1024)).toBe("5.0 Mio");
    expect(formatEta(null)).toBe("—");
    expect(formatEta(75)).toBe("1 min 15 s");
    expect(formatEta(3700)).toBe("1 h 1 min");
    expect(formatRate(2048)).toBe("2.0 Kio/s");
    expect(formatRate(0)).toBe("—");
    expect(progressPercent(50, 200)).toBe(25);
    expect(progressPercent(5, 0)).toBe(0);
  });
  it("permissions", () => {
    expect(modeString(0o754)).toBe("rwxr-xr--");
    expect(modeOctal(0o644)).toBe("644");
    expect(parseOctalMode("0755")).toBe(0o755);
    expect(parseOctalMode("999")).toBeNull();
    expect(parseOctalMode("7")).toBeNull();
  });
});

describe("tri et filtre", () => {
  const list = [e("b.txt", "file", 5), e("A", "dir"), e(".hid", "file"), e("a.txt", "file", 9), e("lnk", "symlink")];
  it("dossiers d'abord, cachés masqués, casse ignorée", () => {
    const out = sortEntries([...list], "name", false, false, "").map((x) => x.name);
    expect(out).toEqual(["A", "a.txt", "b.txt", "lnk"]);
  });
  it("cachés visibles, tri par taille décroissant, filtre", () => {
    expect(sortEntries([...list], "size", true, true, "").map((x) => x.name)[1]).toBe("a.txt");
    expect(sortEntries([...list], "name", false, true, "txt").map((x) => x.name)).toEqual(["a.txt", "b.txt"]);
  });
  it("lien vers dossier traité comme dossier", () => {
    const link: FileEntry = { ...e("l", "symlink"), link_to_dir: true };
    expect(sortEntries([e("z", "file"), link], "name", false, true, "")[0].name).toBe("l");
  });
});

describe("barre de chemin", () => {
  it("normalise", () => {
    expect(normalizePath("/a//b/./c/../d")).toBe("/a/b/d");
    expect(normalizePath("/../..")).toBe("/");
    expect(normalizePath("/")).toBe("/");
  });
  it("résout absolu, relatif et ~", () => {
    expect(resolvePath("/etc/ssh", "/home/u", "/home/u")).toBe("/etc/ssh");
    expect(resolvePath("logs", "/var", "/home/u")).toBe("/var/logs");
    expect(resolvePath("../tmp", "/var/log", "/home/u")).toBe("/var/tmp");
    expect(resolvePath("~", "/var", "/home/u")).toBe("/home/u");
    expect(resolvePath("~/docs", "/var", "/home/u")).toBe("/home/u/docs");
    expect(resolvePath("   ", "/var", "/home/u")).toBe("/var");
    expect(resolvePath("/a/b/", "/", "/")).toBe("/a/b");
  });
});
