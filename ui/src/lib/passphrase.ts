// Estimateur de force de passphrase (feedback UI, jamais de secret journalisé).

export interface Strength {
  score: 0 | 1 | 2 | 3;
  /** Clé i18n du libellé (à passer à t()). */
  labelKey: string;
}

export function estimate(passphrase: string): Strength {
  if (passphrase.length < 8) {
    return { score: 0, labelKey: "passphrase.tooShort" };
  }
  let variety = 0;
  if (/[a-z]/.test(passphrase)) variety += 1;
  if (/[A-Z]/.test(passphrase)) variety += 1;
  if (/[0-9]/.test(passphrase)) variety += 1;
  if (/[^a-zA-Z0-9]/.test(passphrase)) variety += 1;
  if (passphrase.length >= 16 && variety >= 3) {
    return { score: 3, labelKey: "passphrase.excellent" };
  }
  if (passphrase.length >= 12 && variety >= 2) {
    return { score: 2, labelKey: "passphrase.good" };
  }
  return { score: 1, labelKey: "passphrase.ok" };
}
