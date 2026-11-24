// Store des transferts SFTP (M5) : alimenté par l'événement `transfer://event`,
// partagé entre le panneau SFTP et la barre d'état.
import { sftpApi, type TransferInfo } from "./api";

export const transfers = $state<{ items: Record<number, TransferInfo> }>({ items: {} });

let started = false;

export async function initTransfers(): Promise<void> {
  if (started) return;
  started = true;
  try {
    for (const t of await sftpApi.transfers()) transfers.items[t.id] = t;
  } catch {
    // coffre verrouillé / backend indisponible : la liste se remplira aux événements
  }
  await sftpApi.onTransfer((info) => {
    transfers.items[info.id] = info;
  });
}

export function transferList(): TransferInfo[] {
  return Object.values(transfers.items).sort((a, b) => a.id - b.id);
}

export function activeTransferCount(): number {
  return transferList().filter((t) => t.state === "running" || t.state === "queued").length;
}

/** Retire localement les transferts terminés (le backend les oublie via clear). */
export function dropFinished(): void {
  for (const t of transferList()) {
    if (t.state === "done" || t.state === "cancelled" || t.state === "error") {
      delete transfers.items[t.id];
    }
  }
}
