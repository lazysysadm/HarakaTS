// État de joignabilité des serveurs (pastilles de l'arbre) : alimenté par `status://update`.
import { statusApi } from "./api";

export const serverStatus = $state<{ enabled: boolean; map: Record<string, boolean> }>({
  enabled: false,
  map: {},
});

let started = false;

export async function initStatus(): Promise<void> {
  if (started) return;
  started = true;
  await statusApi
    .onUpdate((m) => {
      serverStatus.map = m;
    })
    .catch(() => undefined);
  try {
    serverStatus.map = await statusApi.snapshot();
  } catch {
    // backend indisponible : les pastilles apparaîtront aux événements
  }
}

/** « up » / « down » / « unknown » (non testé : rebond SSH, cycle pas encore passé). */
export function statusOf(id: string): "up" | "down" | "unknown" {
  const v = serverStatus.map[id];
  return v === undefined ? "unknown" : v ? "up" : "down";
}
