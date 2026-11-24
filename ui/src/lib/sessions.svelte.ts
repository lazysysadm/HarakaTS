// Store des sessions ouvertes : liste (événements, sans polling), session « active »
// (dernier terminal focalisé ou choisi dans la barre), partagé par la barre de
// sessions du tableau de bord et le panneau SFTP.
import { api, sftpApi, type SessionInfo } from "./api";

export const sessions = $state<{
  list: SessionInfo[];
  active: string | null;
  openedSeq: number;
  /** Dernier répertoire annoncé par chaque shell (OSC 7) : survit aux changements d'onglet. */
  cwd: Record<string, string>;
}>({
  list: [],
  active: null,
  openedSeq: 0,
  cwd: {},
});

let started = false;

export async function refreshSessions(): Promise<void> {
  try {
    const next = await api.sessionList();
    next.sort((a, b) => a.title.localeCompare(b.title) || a.id.localeCompare(b.id));
    sessions.list = next;
    for (const id of Object.keys(sessions.cwd)) {
      if (!next.some((s) => s.id === id)) delete sessions.cwd[id];
    }
    if (sessions.active && !next.some((s) => s.id === sessions.active)) {
      sessions.active = next.find((s) => s.state === "connected")?.id ?? next[0]?.id ?? null;
    }
  } catch {
    // coffre verrouillé / backend indisponible : liste inchangée
  }
}

export async function initSessions(): Promise<void> {
  if (started) return;
  started = true;
  await refreshSessions();
  const swallow = () => {
    // événements indisponibles : la liste se rafraîchit au prochain événement ou à l'ouverture
  };
  await api
    .onSessionEvent("opened", (id) => {
      sessions.active = id;
      sessions.openedSeq++;
      void refreshSessions();
    })
    .catch(swallow);
  await sftpApi
    .onCwd((session, path) => {
      sessions.cwd[session] = path;
    })
    .catch(swallow);
  await api
    .onSessionEvent("closed", () => void refreshSessions())
    .catch(swallow);
  await api
    .onSessionEvent("active", (id) => {
      sessions.active = id;
      void refreshSessions();
    })
    .catch(swallow);
  await api
    .onSessionEvent("state", () => void refreshSessions())
    .catch(swallow);
}

export function stopSessions(): void {
  started = false;
}

export function connectedSessions(): SessionInfo[] {
  return sessions.list.filter((s) => s.state === "connected");
}

/** Choisit la session active ET ramène sa fenêtre terminal au premier plan. */
export async function activateSession(id: string, focusTerminal = true): Promise<void> {
  sessions.active = id;
  if (focusTerminal) {
    try {
      await api.sessionFocusVte(id);
    } catch {
      // fenêtre déjà fermée : la liste se rafraîchira
    }
  }
}
