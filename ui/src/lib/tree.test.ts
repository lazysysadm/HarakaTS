import { describe, expect, it } from "vitest";
import type { TreeDto } from "./api";
import { descendantIds, flattenTree } from "./tree";

function tree(): TreeDto {
  return {
    folders: [
      {
        id: "b",
        parent_id: "a",
        name: "B",
        color: null,
        icon: null,
        sort_order: 1,
        tags: [],
        defaults: {
          user: null,
          port: null,
          credential_id: null,
          jump_chain: [],
          clipboard_policy: null,
          keepalive_s: null,
          encoding: null,
          auth_method_order: null,
        },
      },
      {
        id: "a",
        parent_id: null,
        name: "A",
        color: null,
        icon: null,
        sort_order: 0,
        tags: [],
        defaults: {
          user: null,
          port: null,
          credential_id: null,
          jump_chain: [],
          clipboard_policy: null,
          keepalive_s: null,
          encoding: null,
          auth_method_order: null,
        },
      },
    ],
    connections: [
      {
        id: "c1",
        folder_id: "b",
        name: "zeta",
        protocol: "ssh",
        host: "h",
        port: null,
        user: null,
        credential_id: null,
        jump_chain: null,
        clipboard_policy: null,
        keepalive_s: null,
        encoding: null,
        color: null,
        icon: null,
        tags: [],
        favorite: false,
        tunnels: [],
        startup_snippet_id: null,
        session_logging: false,
        sftp: { follow_terminal_cwd: false, show_hidden: false },
        notes: "",
        created_at: "",
        updated_at: "",
        last_used_at: null,
      },
      {
        id: "c2",
        folder_id: "a",
        name: "alpha",
        protocol: "ssh",
        host: "h",
        port: null,
        user: null,
        credential_id: null,
        jump_chain: null,
        clipboard_policy: null,
        keepalive_s: null,
        encoding: null,
        color: null,
        icon: null,
        tags: [],
        favorite: true,
        tunnels: [],
        startup_snippet_id: null,
        session_logging: false,
        sftp: { follow_terminal_cwd: false, show_hidden: false },
        notes: "",
        created_at: "",
        updated_at: "",
        last_used_at: null,
      },
    ],
    credentials: [],
    macros: [],
    recents: [],
    ui_state: {},
  };
}

describe("flattenTree", () => {
  it("respecte l'ordre et la profondeur, dossiers expandés seulement", () => {
    const t = tree();
    const collapsed = flattenTree(t, new Set());
    expect(collapsed.map((n) => n.id)).toEqual(["a"]);
    const partial = flattenTree(t, new Set(["a"]));
    expect(partial.map((n) => n.id)).toEqual(["a", "c2", "b"]);
    const full = flattenTree(t, new Set(["a", "b"]));
    expect(full.map((n) => n.id)).toEqual(["a", "c2", "b", "c1"]);
    expect(full[3].depth).toBe(2);
    expect(full[1].parentId).toBe("a");
  });
});

describe("descendantIds", () => {
  it("collecte le sous-arbre pour l'anti-cycle côté UI", () => {
    const ids = descendantIds(tree(), "a");
    expect(ids.has("a")).toBe(true);
    expect(ids.has("b")).toBe(true);
    expect(ids.size).toBe(2);
  });
});
