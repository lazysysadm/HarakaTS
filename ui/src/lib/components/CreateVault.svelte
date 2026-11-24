<script lang="ts">
  import { api, message } from "../api";
  import { appState } from "../state.svelte";
  import { estimate } from "../passphrase";
  import { t } from "../i18n.svelte";

  let master = $state("");
  let confirm = $state("");
  let keyringCache = $state(false);
  const strength = $derived(estimate(master));

  async function submit() {
    appState.busy = true;
    appState.error = null;
    try {
      appState.status = await api.create(master, confirm, keyringCache);
      master = "";
      confirm = "";
    } catch (e) {
      appState.error = message(e);
    } finally {
      appState.busy = false;
    }
  }
</script>

<section class="card">
  <h1>HarakaTS — {t("createvault.title")}</h1>
  <p class="muted">
    {t("createvault.intro")}
  </p>
  <form
    onsubmit={(e) => {
      e.preventDefault();
      void submit();
    }}
  >
    <label for="master">{t("unlock.master")}</label>
    <input id="master" type="password" bind:value={master} autocomplete="new-password" />
    <p class="muted">{t("createvault.strength")} {t(strength.labelKey)}</p>
    <label for="confirm">{t("createvault.confirm")}</label>
    <input id="confirm" type="password" bind:value={confirm} autocomplete="new-password" />
    <label class="row">
      <input
        type="checkbox"
        bind:checked={keyringCache}
        disabled={!appState.status?.keyring_available}
      />
      {t("createvault.keyringCache")}
    </label>
    {#if !appState.status?.keyring_available}
      <p class="muted">
        {t("createvault.noKeyring")}
      </p>
    {/if}
    <button
      type="submit"
      disabled={appState.busy || master.length < 8 || master !== confirm}
    >
      {t("createvault.submit")}
    </button>
  </form>
</section>
