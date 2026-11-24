// État du multi-exec côté interface : alimenté par `multiexec://state`,
// partagé entre le bandeau rouge du tableau de bord et le panneau.
import { multiExecApi } from "./api";

export const multiExec = $state<{ enabled: boolean; sessions: string[] }>({
  enabled: false,
  sessions: [],
});

let started = false;

export async function initMultiExec(): Promise<void> {
  if (started) return;
  started = true;
  try {
    Object.assign(multiExec, await multiExecApi.state());
  } catch {
    // backend indisponible (coffre verrouillé) : l'état arrivera par événement
  }
  await multiExecApi.onState((s) => Object.assign(multiExec, s));
}
