// Utilitaires d'arbre (jalon M2) : aplatissement pour le rendu et le clavier.
import type { Folder, TreeDto } from "./api";

export interface FlatNode {
  kind: "folder" | "connection";
  id: string;
  name: string;
  depth: number;
  parentId: string | null;
  color: string | null;
}

export function sortFolders(a: Folder, b: Folder): number {
  return a.sort_order - b.sort_order || a.name.localeCompare(b.name);
}

export function flattenTree(tree: TreeDto, expanded: ReadonlySet<string>): FlatNode[] {
  const out: FlatNode[] = [];
  const walk = (parentId: string | null, depth: number): void => {
    const folders = tree.folders.filter((f) => f.parent_id === parentId).sort(sortFolders);
    for (const f of folders) {
      out.push({ kind: "folder", id: f.id, name: f.name, depth, parentId, color: f.color });
      if (expanded.has(f.id)) {
        const conns = tree.connections
          .filter((c) => c.folder_id === f.id)
          .sort((a, b) => a.name.localeCompare(b.name));
        for (const c of conns) {
          out.push({
            kind: "connection",
            id: c.id,
            name: c.name,
            depth: depth + 1,
            parentId: f.id,
            color: c.color,
          });
        }
        walk(f.id, depth + 1);
      }
    }
  };
  walk(null, 0);
  return out;
}

export function descendantIds(tree: TreeDto, rootId: string): Set<string> {
  const out = new Set<string>([rootId]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const f of tree.folders) {
      if (f.parent_id && out.has(f.parent_id) && !out.has(f.id)) {
        out.add(f.id);
        grew = true;
      }
    }
  }
  return out;
}
