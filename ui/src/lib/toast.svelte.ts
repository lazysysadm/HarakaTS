// Retour visuel bref après une action de l'utilisateur (enregistrement, suppression, copie…).
export interface Toast {
  id: number;
  text: string;
}

export const toasts = $state<{ list: Toast[] }>({ list: [] });

let nextId = 1;
const DURATION_MS = 2500;
const MAX_VISIBLE = 4;

/** Affiche un message de confirmation, retiré automatiquement après 2,5 s. */
export function notify(text: string): void {
  const id = nextId++;
  toasts.list = [...toasts.list, { id, text }].slice(-MAX_VISIBLE);
  setTimeout(() => dismiss(id), DURATION_MS);
}

export function dismiss(id: number): void {
  toasts.list = toasts.list.filter((x) => x.id !== id);
}
