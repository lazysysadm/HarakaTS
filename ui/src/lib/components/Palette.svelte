<script lang="ts">
  import fuzzysort from "fuzzysort";
  import type { TreeDto } from "../api";
  import { t } from "../i18n.svelte";

  interface Props {
    tree: TreeDto;
    onclose: () => void;
    onselect: (connectionId: string) => void;
    onaction: (action: string) => void;
  }
  let { tree, onclose, onselect, onaction }: Props = $props();

  interface Item {
    label: string;
    kind: string;
    id: string;
  }

  const items = $derived<Item[]>([
    ...tree.connections.map((c) => ({ label: `${c.name} — ${c.host}`, kind: "connection", id: c.id })),
    ...tree.folders.map((f) => ({ label: f.name, kind: "folder", id: f.id })),
    ...tree.credentials.map((c) => ({ label: `${c.name} (${c.username})`, kind: "credential", id: c.id })),
    { label: t("palette.lock"), kind: "action", id: "lock" },
    { label: t("palette.settings"), kind: "action", id: "settings" },
    { label: t("palette.transfer"), kind: "action", id: "transfer" },
    { label: t("palette.audit"), kind: "action", id: "audit" },
  ]);

  function kindLabel(kind: string): string {
    switch (kind) {
      case "connection": return t("palette.kind.connection");
      case "folder": return t("palette.kind.folder");
      case "credential": return t("palette.kind.credential");
      default: return t("palette.kind.action");
    }
  }

  let query = $state("");
  let active = $state(0);
  const results = $derived(
    query.trim()
      ? fuzzysort.go(query, items, { key: "label", limit: 12 }).map((r) => r.obj)
      : items.slice(0, 12),
  );

  function choose(item: Item | undefined) {
    if (!item) return;
    if (item.kind === "connection") onselect(item.id);
    else if (item.kind === "action") onaction(item.id);
    else if (item.kind === "credential") onaction("credentials");
    else onaction("connections");
    onclose();
  }

  function onKeydown(ev: KeyboardEvent) {
    if (ev.key === "ArrowDown") {
      ev.preventDefault();
      active = Math.min(results.length - 1, active + 1);
    } else if (ev.key === "ArrowUp") {
      ev.preventDefault();
      active = Math.max(0, active - 1);
    } else if (ev.key === "Enter") {
      ev.preventDefault();
      choose(results[active]);
    } else if (ev.key === "Escape") {
      onclose();
    }
  }
</script>

<div
  class="palette-overlay"
  role="presentation"
  onclick={(ev) => { if (ev.target === ev.currentTarget) onclose(); }}
  onkeydown={(ev) => { if (ev.key === "Escape") onclose(); }}
>
  <div class="palette" role="dialog" tabindex="-1" aria-label={t("palette.placeholder")}>
    <!-- svelte-ignore a11y_autofocus -->
    <input
      type="text"
      autofocus
      placeholder={t("palette.placeholder")}
      bind:value={query}
      onkeydown={onKeydown}
      oninput={() => (active = 0)}
    />
    <ul>
      {#each results as item, i (item.kind + item.id)}
        <li>
          <button type="button" class:active={i === active} onclick={() => choose(item)}>
            <span class="badge">{kindLabel(item.kind)}</span>
            {item.label}
          </button>
        </li>
      {/each}
    </ul>
  </div>
</div>
