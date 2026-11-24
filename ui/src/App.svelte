<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "./lib/api";
  import { t } from "./lib/i18n.svelte";
  import { appState, uiPrefs } from "./lib/state.svelte";
  import CreateVault from "./lib/components/CreateVault.svelte";
  import Unlock from "./lib/components/Unlock.svelte";
  import Dashboard from "./lib/components/Dashboard.svelte";
  import Toasts from "./lib/components/Toasts.svelte";

  $effect(() => {
    document.documentElement.dataset.theme = uiPrefs.theme;
  });

  onMount(() => {
    void refresh();
    const unlisten = api.onState(() => void refresh());
    const ping = setInterval(() => {
      if (appState.status?.state === "unlocked") void api.activity();
    }, 30_000);
    return () => {
      void unlisten.then((un) => un());
      clearInterval(ping);
    };
  });

  async function refresh() {
    try {
      appState.status = await api.status();
    } catch (e) {
      appState.error = String(e);
    }
  }
</script>

<main class:app-main={appState.status?.state === "unlocked"}>
  {#if appState.status === null}
    <p class="muted">{t("common.loading")}</p>
  {:else if appState.status.state === "uninitialized"}
    <CreateVault />
  {:else if appState.status.state === "locked"}
    <Unlock />
  {:else}
    <Dashboard />
  {/if}
  {#if appState.error}
    <p class="error" role="alert">{appState.error}</p>
  {/if}
</main>
<Toasts />
