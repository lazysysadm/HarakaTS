import { describe, expect, it } from "vitest";
import { estimate } from "./passphrase";

describe("estimate", () => {
  it("refuse les passphrases trop courtes", () => {
    expect(estimate("court").score).toBe(0);
    expect(estimate("court").labelKey).toBe("passphrase.tooShort");
    expect(estimate("").score).toBe(0);
  });

  it("note une passphrase courte mais valide", () => {
    expect(estimate("abcdefgh").score).toBe(1);
  });

  it("note une passphrase longue et variée", () => {
    expect(estimate("Phrase-Longue-2026!").score).toBe(3);
  });

  it("note une passphrase moyenne", () => {
    expect(estimate("Motdepasse12").score).toBe(2);
  });
});
