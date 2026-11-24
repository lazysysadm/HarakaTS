// Régression : « Recharger » doit remplacer le contenu affiché par celui du serveur.
import { beforeEach, describe, expect, it, vi } from "vitest";

const state = { content: "version 1\n", reloads: 0 };
const info = {
  id: "d1",
  session: "s1",
  remote_path: "/srv/a.txt",
  name: "a.txt",
  size: 10,
  mtime: 1,
  mode: 0o644,
  line_ending: "lf",
  utf8: true,
  external: false,
};

vi.mock("@tauri-apps/api/core", () => ({
  invoke: async (cmd: string) => {
    if (cmd === "edit_open") return { info, content: state.content };
    if (cmd === "edit_reload") {
      state.reloads++;
      return { info, content: state.content };
    }
    if (cmd === "edit_list") return [];
    return null;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));

describe("rechargement d'un document", () => {
  beforeEach(() => {
    state.content = "version 1\n";
    state.reloads = 0;
  });

  it("remplace le contenu et incrémente la révision", async () => {
    const { editor, openRemote, reload, setCurrent, currentRev } = await import("./edit.svelte");
    await openRemote("s1", "/srv/a.txt");
    const id = "d1";
    expect(editor.docs[id].current).toBe("version 1\n");
    const rev0 = editor.docs[id].rev;

    // modifications locales non enregistrées…
    setCurrent(id, "version 1 + edits locaux\n");
    // …puis le fichier change sur le serveur et l'utilisateur recharge
    state.content = "version 2 du serveur\n";
    await reload(id);

    expect(state.reloads).toBe(1);
    expect(editor.docs[id].current).toBe("version 2 du serveur\n");
    expect(editor.docs[id].baseline).toBe("version 2 du serveur\n");
    expect(editor.docs[id].rev).toBeGreaterThan(rev0);
    // Miroir non réactif lu par le garde-fou du démontage de l'éditeur : à jour immédiatement.
    expect(currentRev(id)).toBe(editor.docs[id].rev);
  });
});
