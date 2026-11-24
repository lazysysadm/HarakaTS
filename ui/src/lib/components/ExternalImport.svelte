<script lang="ts">
  import { t, tn } from "../i18n.svelte";
  import {
    importExternalApi,
    message,
    type ExternalPreview,
    type ExternalReport,
    type ExternalSource,
    type ImportWarning,
  } from "../api";

  interface Props {
    onrefresh: () => void | Promise<void>;
    onerror: (msg: string) => void;
  }
  let { onrefresh, onerror }: Props = $props();

  const MAX_ROWS = 200;

  let preview = $state<ExternalPreview | null>(null);
  let report = $state<ExternalReport | null>(null);
  let busy = $state(false);

  async function run<T>(fn: () => Promise<T>): Promise<T | null> {
    busy = true;
    try {
      return await fn();
    } catch (e) {
      onerror(message(e));
      return null;
    } finally {
      busy = false;
    }
  }

  async function pick(source: ExternalSource, directory = false) {
    const p = await run(() => importExternalApi.preview(source, directory));
    if (p) {
      preview = p;
      report = null;
    }
  }

  async function apply() {
    const p = preview;
    if (!p) return;
    const r = await run(() => importExternalApi.apply(p.preview_id));
    if (r) {
      report = r;
      preview = null;
      await onrefresh();
    }
  }

  function warningText(w: ImportWarning): string {
    const params: Record<string, string> = { ...w.params };
    if (w.code === "key_unreadable" && params.reason) {
      params.reason = t(`import.keyReason.${params.reason}`);
    }
    const key = `import.warn.${w.code}`;
    const text = t(key, params);
    return text === key ? t("import.warn.unknown", { code: w.code }) : text;
  }

  function keyStatus(status: string): string {
    return t(`import.keys.${status}`);
  }
</script>

<section class="card">
  <h2>{t("import.title")}</h2>
  <p class="muted">{t("import.hint")}</p>
  <div class="row">
    <button type="button" disabled={busy} onclick={() => void pick("ssh_config")}>
      {t("import.src.ssh_config")}
    </button>
    <button type="button" disabled={busy} onclick={() => void pick("remmina")}>
      {t("import.src.remmina")}
    </button>
    <button type="button" disabled={busy} onclick={() => void pick("remmina", true)}>
      {t("import.src.remminaDir")}
    </button>
    <button type="button" disabled={busy} onclick={() => void pick("mobaxterm")}>
      {t("import.src.mobaxterm")}
    </button>
  </div>

  {#if preview}
    <h3>{t("import.preview.title", { folder: preview.root_folder })}</h3>
    <p>
      {tn("import.preview.connections", preview.connections.length)} ·
      {tn("import.preview.folders", preview.folders.length)} ·
      {tn("import.preview.credentials", preview.credentials)}
      {#if preview.skipped > 0}
        · {tn("import.preview.skipped", preview.skipped)}
      {/if}
    </p>
    {#if preview.secrets_to_reenter > 0}
      <p class="muted">{tn("import.preview.reenter", preview.secrets_to_reenter)}</p>
    {/if}
    {#if preview.connections.length === 0}
      <p class="muted">{t("import.nothing")}</p>
    {:else}
      <table>
        <thead>
          <tr>
            <th>{t("import.col.name")}</th>
            <th>{t("import.col.host")}</th>
            <th>{t("import.col.user")}</th>
            <th>{t("import.col.folder")}</th>
          </tr>
        </thead>
        <tbody>
          {#each preview.connections.slice(0, MAX_ROWS) as c, i (i)}
            <tr>
              <td>{c.name}</td>
              <td><code>{c.host}{c.port ? `:${c.port}` : ""}</code></td>
              <td>{c.user ?? ""}</td>
              <td>{c.folder}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if preview.connections.length > MAX_ROWS}
        <p class="muted">{t("import.moreHidden", { n: preview.connections.length - MAX_ROWS })}</p>
      {/if}
    {/if}
    {#if preview.keys.length > 0}
      <h3>{t("import.keys.title")}</h3>
      <ul>
        {#each preview.keys as k (k.path)}
          <li>
            <code>{k.path}</code> — {keyStatus(k.status)}
          </li>
        {/each}
      </ul>
    {/if}
    {#if preview.warnings.length > 0}
      <h3>{tn("import.warnings.title", preview.warnings.length)}</h3>
      <ul class="warnings">
        {#each preview.warnings as w, i (i)}
          <li>{warningText(w)}</li>
        {/each}
      </ul>
    {/if}
    <div class="row">
      <button type="button" disabled={busy || preview.connections.length === 0} onclick={() => void apply()}>
        {t("import.apply")}
      </button>
      <button type="button" class="danger" disabled={busy} onclick={() => (preview = null)}>
        {t("import.cancel")}
      </button>
    </div>
  {/if}

  {#if report}
    <h3>{t("import.report.title")}</h3>
    <p class="ok">
      {tn("import.report.imported", report.imported, { folder: report.root_folder })}
      {#if report.skipped > 0}
        · {tn("import.report.skipped", report.skipped)}
      {/if}
      {#if report.secrets_to_reenter > 0}
        · {tn("import.report.reenter", report.secrets_to_reenter)}
      {/if}
    </p>
    <p class="muted">{t("import.report.reviewHint", { folder: report.root_folder })}</p>
    {#if report.warnings.length > 0}
      <h3>{tn("import.warnings.title", report.warnings.length)}</h3>
      <ul class="warnings">
        {#each report.warnings as w, i (i)}
          <li>{warningText(w)}</li>
        {/each}
      </ul>
    {/if}
  {/if}
</section>

<style>
  section {
    margin-top: 16px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin: 8px 0;
  }
  h3 {
    margin: 12px 0 4px;
    font-size: 13px;
  }
  .warnings {
    margin: 0;
    padding-left: 18px;
    font-size: 12.5px;
    max-height: 180px;
    overflow: auto;
  }
</style>
