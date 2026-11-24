<script lang="ts">
  import fuzzysort from "fuzzysort";
  import { api, message, type TreeDto } from "../api";
  import { descendantIds, flattenTree } from "../tree";
  import { t } from "../i18n.svelte";
  import { serverStatus, statusOf } from "../status.svelte";

  interface Props {
    tree: TreeDto;
    selected: string | null;
    onselect: (id: string) => void;
    onconnect: (id: string) => void;
    onedit: (id: string) => void;
    onrefresh: (tree: TreeDto) => void;
    onerror: (msg: string) => void;
    onnewconnection: (folderId: string | null) => void;
  }
  let { tree, selected, onselect, onconnect, onedit, onrefresh, onerror, onnewconnection }: Props = $props();

  // svelte-ignore state_referenced_locally
  let expanded = $state(new Set(tree.folders.map((f) => f.id)));
  const flat = $derived(flattenTree(tree, expanded));
  const favorites = $derived(tree.connections.filter((c) => c.favorite));
  const favoriteIds = $derived(new Set(favorites.map((c) => c.id)));

  let search = $state("");
  const searchItems = $derived(
    tree.connections.map((c) => ({
      label: `${c.name} ${c.host} ${c.user ?? ""}`,
      id: c.id,
      name: c.name,
      host: c.host,
    })),
  );
  const searchResults = $derived(
    search.trim()
      ? fuzzysort.go(search, searchItems, { key: "label", limit: 30 }).map((r) => r.obj)
      : [],
  );

  let dragItem = $state<{ kind: "folder" | "connection"; id: string } | null>(null);
  let editingId = $state<string | null>(null);
  let renameValue = $state("");
  let pendingDelete = $state<{ kind: "folder" | "connection"; id: string } | null>(null);
  let container: HTMLDivElement | undefined = $state();

  async function run(fn: () => Promise<TreeDto>) {
    try {
      onrefresh(await fn());
    } catch (e) {
      onerror(message(e));
    }
  }

  function toggle(id: string) {
    const next = new Set(expanded);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    expanded = next;
  }

  async function newFolder() {
    await run(() =>
      api.folderUpsert({
        id: "",
        parent_id: null,
        name: t("tree.newFolder"),
        color: null,
        icon: null,
        sort_order: tree.folders.length,
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
      }),
    );
    const created = tree.folders[tree.folders.length - 1];
    if (created) startRename(created.id, created.name);
  }

  function startRename(id: string, name: string) {
    editingId = id;
    renameValue = name;
  }

  async function commitRename() {
    const id = editingId;
    editingId = null;
    if (!id) return;
    const folder = tree.folders.find((f) => f.id === id);
    if (folder && renameValue.trim() && renameValue !== folder.name) {
      await run(() => api.folderUpsert({ ...folder, name: renameValue.trim() }));
    }
  }

  function askDelete(kind: "folder" | "connection", id: string) {
    pendingDelete = { kind, id };
  }

  async function confirmDelete(cascade: boolean) {
    const target = pendingDelete;
    pendingDelete = null;
    if (!target) return;
    if (target.kind === "folder") {
      await run(() => api.folderDelete(target.id, cascade));
    } else {
      await run(() => api.connectionDelete(target.id));
    }
  }

  function onDragStart(kind: "folder" | "connection", id: string, ev: DragEvent) {
    dragItem = { kind, id };
    ev.dataTransfer?.setData("text/plain", JSON.stringify({ kind, id }));
    if (ev.dataTransfer) ev.dataTransfer.effectAllowed = "move";
  }

  function canDrop(targetFolderId: string | null): boolean {
    if (!dragItem) return false;
    if (targetFolderId === null) return dragItem.kind === "folder";
    if (dragItem.kind === "folder") {
      if (dragItem.id === targetFolderId) return false;
      return !descendantIds(tree, dragItem.id).has(targetFolderId);
    }
    return true;
  }

  async function onDrop(targetFolderId: string | null, ev: DragEvent) {
    ev.preventDefault();
    const item = dragItem;
    dragItem = null;
    if (!item || !canDropFor(item, targetFolderId)) return;
    if (item.kind === "folder") {
      await run(() => api.folderMove(item.id, targetFolderId, null));
    } else {
      const conn = tree.connections.find((c) => c.id === item.id);
      if (conn && targetFolderId && conn.folder_id !== targetFolderId) {
        await run(() => api.connectionUpsert({ ...conn, folder_id: targetFolderId }));
      }
    }
  }

  function canDropFor(
    item: { kind: "folder" | "connection"; id: string },
    targetFolderId: string | null,
  ): boolean {
    if (targetFolderId === null) return item.kind === "folder";
    if (item.kind === "folder") {
      return item.id !== targetFolderId && !descendantIds(tree, item.id).has(targetFolderId);
    }
    return true;
  }

  async function toggleFavorite(id: string) {
    const conn = tree.connections.find((c) => c.id === id);
    if (conn) await run(() => api.connectionUpsert({ ...conn, favorite: !conn.favorite }));
  }

  const selectedFolder = $derived(tree.folders.find((f) => f.id === selected) ?? null);
  const moveTargets = $derived(
    selectedFolder
      ? tree.folders.filter(
          (f) =>
            f.id !== selectedFolder.id &&
            !descendantIds(tree, selectedFolder.id).has(f.id) &&
            f.id !== selectedFolder.parent_id,
        )
      : [],
  );

  async function moveSelected(target: string) {
    if (!selectedFolder) return;
    const parent = target === "__root__" ? null : target;
    await run(() => api.folderMove(selectedFolder.id, parent, null));
  }

  function onKeydown(ev: KeyboardEvent) {
    if (ev.key !== "ArrowDown" && ev.key !== "ArrowUp") return;
    const rows = Array.from(container?.querySelectorAll<HTMLElement>(".tree-row") ?? []);
    const idx = rows.findIndex((r) => r === document.activeElement);
    if (idx === -1) return;
    ev.preventDefault();
    const next = ev.key === "ArrowDown" ? idx + 1 : idx - 1;
    rows[Math.max(0, Math.min(rows.length - 1, next))]?.focus();
  }
</script>

<div class="tree" bind:this={container} onkeydown={onKeydown} role="tree" tabindex="-1" aria-label={t("tree.folders")}>
  <div class="tree-toolbar">
    <button type="button" class="secondary" onclick={() => void newFolder()}>+ {t("tree.newFolder")}</button>
    <button type="button" class="secondary" onclick={() => onnewconnection(null)}>+ {t("tree.newConnection")}</button>
  </div>
  {#if selectedFolder}
    <label class="move-label" for="tree-move">{t("tree.moveTo")} « {selectedFolder.name} »</label>
    <select
      id="tree-move"
      value=""
      onchange={(ev) => {
        const v = ev.currentTarget.value;
        ev.currentTarget.value = "";
        void moveSelected(v);
      }}
    >
      <option value="" disabled>—</option>
      <option value="__root__">{t("tree.root")}</option>
      {#each moveTargets as f (f.id)}
        <option value={f.id}>{f.name}</option>
      {/each}
    </select>
  {/if}

  <input
    class="tree-search"
    type="search"
    placeholder={t("tree.search")}
    aria-label={t("tree.search")}
    bind:value={search}
  />

  {#if search.trim()}
    <div class="tree-section">{t("tabs.connections")} ({searchResults.length})</div>
    {#each searchResults as r (r.id)}
      <button
        type="button"
        class="tree-row link"
        class:selected={selected === r.id}
        onclick={() => onselect(r.id)}
      >
        <span class="node-name">{r.name}</span>
        <span class="muted">{r.host}</span>
      </button>
    {/each}
    {#if searchResults.length === 0}
      <p class="muted">—</p>
    {/if}
  {:else}
  {#snippet statusDot(id: string)}
    {#if serverStatus.enabled}
      <span class="srv-dot {statusOf(id)}" title={t(`status.${statusOf(id)}`)} role="img" aria-label={t(`status.${statusOf(id)}`)}></span>
    {/if}
  {/snippet}
  {#snippet quickRow(c: { id: string; name: string; host: string; favorite: boolean })}
    <div
      class="tree-row"
      class:selected={selected === c.id}
      role="treeitem"
      tabindex="0"
      aria-selected={selected === c.id}
      onclick={() => onselect(c.id)}
      ondblclick={(ev) => { ev.stopPropagation(); onconnect(c.id); }}
      onkeydown={(ev) => {
        if (ev.key === "Enter") { ev.preventDefault(); onconnect(c.id); }
        else if (ev.key === " ") { ev.preventDefault(); onselect(c.id); }
      }}
    >
      <span class="expander">·</span>
      {@render statusDot(c.id)}
      <span class="node-name">{c.name}</span>
      <span class="spacer"></span>
      <button type="button" class="icon-btn connect" title={t("tree.connect")} onclick={(ev) => { ev.stopPropagation(); onconnect(c.id); }}>▶</button>
      <button type="button" class="icon-btn" title={t("tree.edit")} onclick={(ev) => { ev.stopPropagation(); onedit(c.id); }}>✎</button>
      <button type="button" class="icon-btn" class:fav-on={c.favorite} title={t("editor.favorite")} onclick={(ev) => { ev.stopPropagation(); void toggleFavorite(c.id); }}>{c.favorite ? "★" : "☆"}</button>
    </div>
  {/snippet}
  {#if favorites.length > 0}
    <div class="tree-section">★ {t("tree.favorites")}</div>
    {#each favorites as c (c.id)}
      {@render quickRow(c)}
    {/each}
  {/if}

  <div class="tree-section">{t("tree.folders")}</div>
  {#each flat as node (node.id)}
    <div
      class="tree-row"
      class:selected={selected === node.id}
      style="padding-left: {8 + node.depth * 16}px"
      role="treeitem"
      tabindex="0"
      aria-selected={selected === node.id}
      aria-expanded={node.kind === "folder" ? expanded.has(node.id) : undefined}
      draggable="true"
      ondragstart={(ev) => onDragStart(node.kind, node.id, ev)}
      ondragend={() => (dragItem = null)}
      ondragover={(ev) => {
        if (node.kind === "folder" && canDrop(node.id)) ev.preventDefault();
      }}
      ondrop={(ev) => {
        if (node.kind === "folder") void onDrop(node.id, ev);
      }}
      onclick={() => (node.kind === "folder" ? toggle(node.id) : onselect(node.id))}
      ondblclick={(ev) => {
        ev.stopPropagation();
        if (node.kind === "folder") {
          startRename(node.id, node.name);
        } else {
          onconnect(node.id);
        }
      }}
      onkeydown={(ev) => {
        if (ev.key === "Enter" || ev.key === " ") {
          ev.preventDefault();
          node.kind === "folder" ? toggle(node.id) : onselect(node.id);
        }
      }}
    >
      {#if node.kind === "folder"}
        <span class="expander">{expanded.has(node.id) ? "▾" : "▸"}</span>
      {:else}
        <span class="expander">·</span>
      {/if}
      {#if node.kind === "connection"}{@render statusDot(node.id)}{/if}
      {#if node.color}<span class="dot" style="background: {node.color}"></span>{/if}
      {#if editingId === node.id}
        <input
          class="rename"
          bind:value={renameValue}
          onblur={() => void commitRename()}
          onkeydown={(ev) => {
            if (ev.key === "Enter") void commitRename();
            if (ev.key === "Escape") editingId = null;
          }}
          onclick={(ev) => ev.stopPropagation()}
        />
      {:else}
        <span class="node-name">{node.name}</span>
      {/if}
      <span class="spacer"></span>
      {#if node.kind === "folder"}
        <button type="button" class="icon-btn" title={t("tree.newConnection")} onclick={(ev) => { ev.stopPropagation(); onnewconnection(node.id); }}>+</button>
        <button type="button" class="icon-btn" title={t("tree.rename")} onclick={(ev) => { ev.stopPropagation(); startRename(node.id, node.name); }}>✎</button>
      {:else}
        <button type="button" class="icon-btn connect" title={t("tree.connect")} onclick={(ev) => { ev.stopPropagation(); onconnect(node.id); }}>▶</button>
        <button type="button" class="icon-btn" title={t("tree.edit")} onclick={(ev) => { ev.stopPropagation(); onedit(node.id); }}>✎</button>
        <button type="button" class="icon-btn" class:fav-on={favoriteIds.has(node.id)} title={t("editor.favorite")} onclick={(ev) => { ev.stopPropagation(); void toggleFavorite(node.id); }}>{favoriteIds.has(node.id) ? "★" : "☆"}</button>
        <button type="button" class="icon-btn" title={t("tree.duplicate")} onclick={(ev) => { ev.stopPropagation(); void run(() => api.connectionDuplicate(node.id)); }}>⧉</button>
      {/if}
      <button type="button" class="icon-btn danger" title={t("tree.delete")} onclick={(ev) => { ev.stopPropagation(); askDelete(node.kind, node.id); }}>🗑</button>
    </div>
  {/each}

  {#if flat.length === 0}
    <p class="muted">{t("tree.empty")}</p>
  {/if}
  {/if}

  {#if pendingDelete}
    <div class="confirm-bar">
      <span>{t("tree.delete")} « {pendingDelete.id.slice(0, 8)}… » ?</span>
      <button type="button" class="danger" onclick={() => void confirmDelete(false)}>{t("common.confirm")}</button>
      <button type="button" class="secondary" onclick={() => void confirmDelete(true)}>{t("tree.cascade")}</button>
      <button type="button" class="secondary" onclick={() => (pendingDelete = null)}>{t("common.cancel")}</button>
    </div>
  {/if}
</div>
