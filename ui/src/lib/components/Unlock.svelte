<script lang="ts">
  import { onMount } from "svelte";
  import { api, message, type BackupDto } from "../api";
  import { appState } from "../state.svelte";
  import { t, formatDateTime } from "../i18n.svelte";

  let master = $state("");
  let backups = $state<BackupDto[]>([]);
  let showBackups = $state(false);
  let corrupt = $state(false);

  onMount(() => {
    void (async () => {
      try {
        if (!(await api.vaultIntegrity())) {
          corrupt = true;
          await loadBackups();
        }
      } catch {
        // contrôle indisponible : le déverrouillage normal reste possible
      }
    })();
  });

  async function unlock(useKeyring: boolean) {
    appState.busy = true;
    appState.error = null;
    try {
      appState.status = await api.unlock(useKeyring ? null : master, useKeyring);
      master = "";
    } catch (e) {
      appState.error = message(e);
    } finally {
      appState.busy = false;
    }
  }

  async function loadBackups() {
    appState.error = null;
    try {
      backups = await api.listBackups();
      showBackups = true;
    } catch (e) {
      appState.error = message(e);
    }
  }

  async function restore(index: number) {
    appState.busy = true;
    appState.error = null;
    try {
      appState.status = await api.restoreBackup(index);
      showBackups = false;
    } catch (e) {
      appState.error = message(e);
    } finally {
      appState.busy = false;
    }
  }
</script>

<section class="card">
  <h1>{t("unlock.title")}</h1>
  <p class="muted">{t("unlock.data")} <code>{appState.status?.data_dir}</code></p>
  <form
    onsubmit={(e) => {
      e.preventDefault();
      void unlock(false);
    }}
  >
    <label for="master">{t("unlock.master")}</label>
    <input id="master" type="password" bind:value={master} autocomplete="current-password" />
    <button type="submit" disabled={appState.busy || master.length === 0}>
      {t("unlock.unlock")}
    </button>
    {#if appState.status?.keyring_available}
      <button
        type="button"
        class="secondary"
        disabled={appState.busy}
        onclick={() => void unlock(true)}
      >
        {t("unlock.viaKwallet")}
      </button>
    {/if}
  </form>

  {#if corrupt}<p class="error">{t("unlock.corrupt")}</p>{/if}
  <h3>{t("unlock.recoveryTitle")}</h3>
  <p class="muted">
    {t("unlock.recoveryInfo")}
  </p>
  <button type="button" class="secondary" onclick={() => void loadBackups()}>
    {t("unlock.showBackups")}
  </button>
  {#if showBackups}
    {#if backups.length === 0}
      <p class="muted">{t("unlock.noBackups")}</p>
    {:else}
      <table>
        <thead>
          <tr><th>#</th><th>{t("unlock.date")}</th><th></th></tr>
        </thead>
        <tbody>
          {#each backups as b (b.index)}
            <tr>
              <td>{b.index}</td>
              <td>
                {b.modified_epoch
                  ? formatDateTime(b.modified_epoch)
                  : "—"}
              </td>
              <td>
                <button
                  type="button"
                  class="secondary"
                  disabled={appState.busy || !b.valid}
                  title={b.valid ? "" : t("unlock.backupInvalid")}
                  onclick={() => void restore(b.index)}
                >
                  {t("unlock.restore")}
                </button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  {/if}
</section>
