<script lang="ts">
  import type { Snippet } from "svelte";
  import { t } from "../i18n.svelte";
  import {
    breadcrumbs,
    formatDate,
    formatSize,
    isDirLike,
    resolvePath,
    modeString,
    sortEntries,
    type FileEntry,
    type SortKey,
  } from "../sftp";

  interface Props {
    title: string;
    path: string;
    entries: FileEntry[];
    loading: boolean;
    error: string | null;
    selected: Set<string>;
    /** Données de glisser-déposer : identifie le panneau d'origine. */
    side: "local" | "remote";
    /** Dossier personnel (pour « ~ » dans la barre de chemin). */
    home?: string;
    /** Boutons d'action affichés sous l'en-tête du panneau. */
    actions?: Snippet;
    onnavigate: (path: string) => void;
    onrefresh: () => void;
    onselect: (selected: Set<string>) => void;
    ondropfrom: (fromSide: "local" | "remote", paths: string[]) => void;
    onactivate: (entry: FileEntry) => void;
  }
  let {
    title,
    path,
    entries,
    loading,
    error,
    selected,
    side,
    home = "/",
    actions,
    onnavigate,
    onrefresh,
    onselect,
    ondropfrom,
    onactivate,
  }: Props = $props();

  let sortKey = $state<SortKey>("name");
  let desc = $state(false);
  let showHidden = $state(false);
  let filter = $state("");
  let anchor: string | null = null;
  let dragOver = $state(false);
  // Barre de chemin éditable : suit le dossier affiché, Entrée navigue.
  // svelte-ignore state_referenced_locally
  let pathText = $state(path);
  $effect(() => {
    pathText = path;
  });

  function go() {
    const target = resolvePath(pathText, path, home);
    if (target !== path) onnavigate(target);
    else pathText = path;
  }

  const rows = $derived(sortEntries([...entries], sortKey, desc, showHidden, filter));
  const crumbs = $derived(breadcrumbs(path));

  function sortBy(key: SortKey) {
    if (sortKey === key) desc = !desc;
    else {
      sortKey = key;
      desc = false;
    }
  }

  function click(ev: MouseEvent, e: FileEntry) {
    const next = new Set(selected);
    if (ev.shiftKey && anchor) {
      const names = rows.map((r) => r.path);
      const a = names.indexOf(anchor);
      const b = names.indexOf(e.path);
      if (a >= 0 && b >= 0) {
        next.clear();
        for (const p of names.slice(Math.min(a, b), Math.max(a, b) + 1)) next.add(p);
      }
    } else if (ev.ctrlKey || ev.metaKey) {
      if (next.has(e.path)) next.delete(e.path);
      else next.add(e.path);
      anchor = e.path;
    } else {
      next.clear();
      next.add(e.path);
      anchor = e.path;
    }
    onselect(next);
  }

  function dragStart(ev: DragEvent, e: FileEntry) {
    const paths = selected.has(e.path) ? [...selected] : [e.path];
    ev.dataTransfer?.setData("application/x-haraka-files", JSON.stringify({ side, paths }));
    if (ev.dataTransfer) ev.dataTransfer.effectAllowed = "copy";
  }

  function drop(ev: DragEvent) {
    dragOver = false;
    const raw = ev.dataTransfer?.getData("application/x-haraka-files");
    if (!raw) return;
    ev.preventDefault();
    try {
      const data = JSON.parse(raw) as { side: "local" | "remote"; paths: string[] };
      if (data.side !== side) ondropfrom(data.side, data.paths);
    } catch {
      // charge utile inattendue : ignorée
    }
  }

  function arrow(key: SortKey): string {
    return sortKey === key ? (desc ? " ▾" : " ▴") : "";
  }
</script>

<section
  class="file-pane"
  class:drag-over={dragOver}
  aria-label={title}
  ondragover={(ev) => {
    if (ev.dataTransfer?.types.includes("application/x-haraka-files")) {
      ev.preventDefault();
      dragOver = true;
    }
  }}
  ondragleave={() => (dragOver = false)}
  ondrop={drop}
>
  <div class="pane-head">
    <strong>{title}</strong>
    <span class="spacer"></span>
    <label class="inline small"><input type="checkbox" bind:checked={showHidden} /> {t("pane.hidden")}</label>
    <button type="button" class="icon-btn always" title={t("pane.parent")} onclick={() => onnavigate(path.replace(/\/+$/, "").replace(/\/[^/]*$/, "") || "/")}>↑</button>
    <button type="button" class="icon-btn always" title={t("pane.refresh")} onclick={onrefresh}>⟳</button>
  </div>

  <input
    type="text"
    class="path-input mono"
    spellcheck="false"
    autocomplete="off"
    aria-label={t("pane.pathAria", { title })}
    placeholder={t("pane.pathPlaceholder")}
    bind:value={pathText}
    onkeydown={(ev) => {
      if (ev.key === "Enter") go();
      else if (ev.key === "Escape") pathText = path;
    }}
    onfocus={(ev) => ev.currentTarget.select()}
  />

  {#if actions}
    <div class="pane-tools">{@render actions()}</div>
  {/if}

  <div class="crumbs">
    {#each crumbs as c, i (c.path)}
      <button type="button" class="crumb" onclick={() => onnavigate(c.path)}>{i === 0 ? "/" : c.label}</button>{#if i > 0 && i < crumbs.length - 1}<span class="muted">/</span>{/if}
    {/each}
  </div>

  <input type="text" class="pane-filter" placeholder={t("pane.filter")} bind:value={filter} aria-label={t("pane.filterAria", { title })} />

  <div class="pane-list">
    {#if error}
      <p class="error">{error}</p>
    {:else if loading}
      <p class="muted">{t("common.loading")}</p>
    {:else}
      <table class="file-table">
        <thead>
          <tr>
            <th><button type="button" class="th-btn" onclick={() => sortBy("name")}>{t("pane.name")}{arrow("name")}</button></th>
            <th class="num"><button type="button" class="th-btn" onclick={() => sortBy("size")}>{t("pane.size")}{arrow("size")}</button></th>
            <th><button type="button" class="th-btn" onclick={() => sortBy("mtime")}>{t("pane.modified")}{arrow("mtime")}</button></th>
            <th>{t("pane.perms")}</th>
          </tr>
        </thead>
        <tbody>
          {#each rows as e (e.path)}
            <tr
              class:selected={selected.has(e.path)}
              draggable="true"
              ondragstart={(ev) => dragStart(ev, e)}
              onclick={(ev) => click(ev, e)}
              ondblclick={() => onactivate(e)}
            >
              <td class="name">
                <span class="ico">{isDirLike(e) ? "📁" : e.kind === "symlink" ? "🔗" : "📄"}</span>
                {e.name}{#if e.kind === "symlink" && e.link_target}<span class="muted"> → {e.link_target}</span>{/if}
              </td>
              <td class="num muted">{isDirLike(e) ? "" : formatSize(e.size)}</td>
              <td class="muted">{formatDate(e.mtime)}</td>
              <td class="muted mono">{modeString(e.mode)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if rows.length === 0}
        <p class="muted">{t("pane.empty")}</p>
      {/if}
    {/if}
  </div>
</section>
