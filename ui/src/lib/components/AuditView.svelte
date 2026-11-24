<script lang="ts">
  import { i18n, t } from "../i18n.svelte";
  import { api, message, type AuditEntry } from "../api";

  interface Props {
    onerror: (msg: string) => void;
  }
  let { onerror }: Props = $props();

  let entries = $state<AuditEntry[]>([]);
  let busy = $state(false);

  async function load() {
    busy = true;
    try {
      entries = await api.audit(50);
    } catch (e) {
      onerror(message(e));
    } finally {
      busy = false;
    }
  }
</script>

<section class="card">
  <h2>{t("audit.title")}</h2>
  <button type="button" class="secondary" disabled={busy} onclick={() => void load()}>
    {t("audit.load")}
  </button>
  {#if entries.length > 0}
    <table>
      <thead>
        <tr><th>{t("audit.colDate")}</th><th>{t("audit.colEvent")}</th><th>{t("audit.colTarget")}</th><th>{t("audit.colDetail")}</th></tr>
      </thead>
      <tbody>
        {#each entries as e, i (i)}
          <tr>
            <td>{new Date(e.ts).toLocaleString(i18n.locale)}</td>
            <td>{e.event}</td>
            <td>{e.target?.slice(0, 8) ?? "—"}</td>
            <td>{e.detail ?? "—"}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>
