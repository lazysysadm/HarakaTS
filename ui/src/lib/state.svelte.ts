import type { VaultStatus } from "./api";

export const appState = $state<{
  status: VaultStatus | null;
  error: string | null;
  busy: boolean;
}>({ status: null, error: null, busy: false });

// Préférences d'interface appliquées globalement (thème : system/gris/bleu/clair).
export const uiPrefs = $state({ theme: "system" });
