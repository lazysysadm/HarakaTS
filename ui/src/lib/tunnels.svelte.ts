// Store des tunnels live (M7) : alimenté par `tunnel://state`, partagé entre le
// panneau et la barre d'outils du tableau de bord.
import { tunnelApi, type TunnelEvent, type TunnelInfo } from "./api";

export const tunnels = $state<{ items: Record<string, TunnelInfo> }>({ items: {} });

const keyOf = (session: string, id: string) => `${session}\u0000${id}`;
let started = false;

export function applyTunnelEvent(ev: TunnelEvent): void {
  if (ev.type === "changed") tunnels.items[keyOf(ev.info.session, ev.info.id)] = ev.info;
  else delete tunnels.items[keyOf(ev.session, ev.id)];
}

export async function initTunnels(): Promise<void> {
  if (started) return;
  started = true;
  await tunnelApi.onState(applyTunnelEvent);
  try {
    for (const info of await tunnelApi.list()) tunnels.items[keyOf(info.session, info.id)] = info;
  } catch {
    // coffre verrouillé : la liste se remplira aux événements
  }
}

export function tunnelList(): TunnelInfo[] {
  return Object.values(tunnels.items).sort((a, b) =>
    a.session === b.session ? a.id.localeCompare(b.id) : a.session.localeCompare(b.session),
  );
}

/** Nombre de tunnels vivants (en écoute ou en démarrage). */
export function liveTunnelCount(): number {
  return tunnelList().filter((x) => x.state !== "error").length;
}

export function tunnelKey(session: string, id: string): string {
  return keyOf(session, id);
}
