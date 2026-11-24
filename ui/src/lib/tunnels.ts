// Helpers purs des tunnels (validation, libellés) — testés sans DOM.
import type { TunnelSpec } from "./api";

export function isLoopback(addr: string): boolean {
  const a = addr.trim().toLowerCase().replace(/^\[|\]$/g, "");
  return a === "" || a === "localhost" || a === "::1" || /^127(\.\d{1,3}){3}$/.test(a);
}

function validPort(n: unknown): boolean {
  return typeof n === "number" && Number.isInteger(n) && n >= 0 && n <= 65535;
}

/** Clé i18n de l'erreur de validation d'une spec, ou null si elle est valide. */
export function specError(s: Pick<TunnelSpec, "kind" | "bind_port" | "dest_host" | "dest_port">): string | null {
  if (!validPort(s.bind_port)) return "tunnel.err.bindPort";
  if (s.kind !== "dynamic") {
    if (!s.dest_host || !s.dest_host.trim()) return "tunnel.err.destHost";
    if (!validPort(s.dest_port) || s.dest_port === 0) return "tunnel.err.destPort";
  }
  return null;
}

export function newSpec(): TunnelSpec {
  const id =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : `t-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
  return { id, kind: "local", bind_addr: "127.0.0.1", bind_port: 8080, dest_host: "localhost", dest_port: 80, auto_start: false };
}

/** Adresse d'écoute affichée (vide = boucle locale). */
export function bindLabel(addr: string, port: number): string {
  const a = addr.trim() || "127.0.0.1";
  return `${a.includes(":") ? `[${a}]` : a}:${port}`;
}

export function destLabel(kind: string, host: string | null, port: number | null): string {
  return kind === "dynamic" ? "SOCKS5" : `${host ?? ""}:${port ?? ""}`;
}
