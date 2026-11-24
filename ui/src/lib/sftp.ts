// Helpers purs du navigateur SFTP (M5) — testés par vitest.
import { formatDateTime, t } from "./i18n.svelte";

export type EntryKind = "dir" | "file" | "symlink" | "other";

export interface FileEntry {
  name: string;
  path: string;
  kind: EntryKind;
  size: number;
  mtime: number | null;
  mode: number | null;
  link_to_dir?: boolean;
  link_target?: string | null;
  user?: string | null;
  group?: string | null;
  uid?: number | null;
  gid?: number | null;
}

export type SortKey = "name" | "size" | "mtime";

/** Un lien vers un dossier se parcourt comme un dossier. */
export function isDirLike(e: FileEntry): boolean {
  return e.kind === "dir" || (e.kind === "symlink" && e.link_to_dir === true);
}

export function joinPath(dir: string, name: string): string {
  return dir.endsWith("/") ? `${dir}${name}` : `${dir}/${name}`;
}

export function parentPath(path: string): string {
  const trimmed = path.length > 1 ? path.replace(/\/+$/, "") : path;
  const idx = trimmed.lastIndexOf("/");
  if (idx <= 0) return "/";
  return trimmed.slice(0, idx);
}

/** Fil d'Ariane : [{label, path}] de la racine au dossier courant. */
export function breadcrumbs(path: string): { label: string; path: string }[] {
  const out = [{ label: "/", path: "/" }];
  let acc = "";
  for (const part of path.split("/").filter(Boolean)) {
    acc += `/${part}`;
    out.push({ label: part, path: acc });
  }
  return out;
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} ${t("unit.b")}`;
  const units = ["unit.kib", "unit.mib", "unit.gib", "unit.tib"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${t(units[i])}`;
}

export function formatRate(bps: number): string {
  return bps > 0 ? t("unit.perSec", { size: formatSize(bps) }) : "—";
}

export function formatEta(seconds: number | null | undefined): string {
  if (seconds == null) return "—";
  const sec = t("unit.sec");
  if (seconds < 60) return `${seconds} ${sec}`;
  const m = Math.floor(seconds / 60);
  if (m < 60) return `${m} ${t("unit.min")} ${seconds % 60} ${sec}`;
  return `${Math.floor(m / 60)} ${t("unit.hour")} ${m % 60} ${t("unit.min")}`;
}

/** Date locale (langue active de l'interface). */
export function formatDate(epoch: number | null): string {
  return formatDateTime(epoch);
}

/** rwxr-xr-x depuis les bits 0o777 (setuid/sticky ignorés à l'affichage). */
export function modeString(mode: number | null): string {
  if (mode == null) return "";
  const bits = "rwxrwxrwx";
  let out = "";
  for (let i = 0; i < 9; i++) out += mode & (1 << (8 - i)) ? bits[i] : "-";
  return out;
}

/** "755" / "0644" → 0o755 ; null si invalide. */
export function parseOctalMode(text: string): number | null {
  const v = text.trim();
  if (!/^[0-7]{3,4}$/.test(v)) return null;
  return parseInt(v, 8);
}

export function modeOctal(mode: number | null): string {
  return mode == null ? "" : (mode & 0o7777).toString(8).padStart(3, "0");
}

/** Tri : dossiers d'abord, puis clé choisie (insensible à la casse pour le nom). */
export function sortEntries(
  entries: FileEntry[],
  key: SortKey,
  desc: boolean,
  showHidden: boolean,
  filter: string,
): FileEntry[] {
  const needle = filter.trim().toLowerCase();
  const list = entries.filter(
    (e) => (showHidden || !e.name.startsWith(".")) && (!needle || e.name.toLowerCase().includes(needle)),
  );
  const dir = desc ? -1 : 1;
  return list.sort((a, b) => {
    const da = isDirLike(a) ? 0 : 1;
    const db = isDirLike(b) ? 0 : 1;
    if (da !== db) return da - db;
    let c = 0;
    if (key === "size") c = a.size - b.size;
    else if (key === "mtime") c = (a.mtime ?? 0) - (b.mtime ?? 0);
    else c = a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true });
    return c * dir || a.name.localeCompare(b.name);
  });
}

export function progressPercent(done: number, total: number): number {
  return total > 0 ? Math.min(100, Math.floor((done / total) * 100)) : 0;
}

/** Normalise un chemin POSIX : « // », « . » et « .. » résolus (jamais au-dessus de /). */
export function normalizePath(path: string): string {
  const out: string[] = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") out.pop();
    else out.push(part);
  }
  return `/${out.join("/")}`;
}

/**
 * Interprète le texte saisi dans la barre de chemin : « ~ » et « ~/x » → dossier
 * personnel, chemin relatif → relatif au dossier affiché, sinon absolu.
 */
export function resolvePath(input: string, current: string, home: string): string {
  const text = input.trim();
  if (!text) return current;
  let p = text;
  if (p === "~") p = home || "/";
  else if (p.startsWith("~/")) p = joinPath(home || "/", p.slice(2));
  else if (!p.startsWith("/")) p = joinPath(current || "/", p);
  return normalizePath(p);
}
