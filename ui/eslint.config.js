import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist/**", "node_modules/**"] },
  ...tseslint.configs.recommended,
  {
    // Les références triple-slash sont idiomatiques dans les fichiers
    // d'environnement Vite/Svelte (vite-env.d.ts).
    files: ["**/*.d.ts"],
    rules: { "@typescript-eslint/triple-slash-reference": "off" },
  },
);
