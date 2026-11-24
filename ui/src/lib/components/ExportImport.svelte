<script lang="ts">
  import { t, tn } from "../i18n.svelte";
  import ExternalImport from "./ExternalImport.svelte";
  import {
    api,
    message,
    type Counts,
    type PreviewDto,
    type TreeDto,
  } from "../api";

  interface Props {
    tree: TreeDto;
    onerror: (msg: string) => void;
    onrefresh?: () => void | Promise<void>;
  }
  let { tree, onerror, onrefresh }: Props = $props();

  let exportScope = $state("all");
  let exportFolder = $state("");
  let exportPass = $state("");
  let exportConfirm = $state("");
  let exportResult = $state<{ path: string; counts: Counts } | null>(null);

  let importPass = $state("");
  let preview = $state<PreviewDto | null>(null);
  let defaultStrategy = $state("keep");
  let strategies = $state<Record<string, string>>({});
  let importReport = $state<Counts | null>(null);
  let busy = $state(false);

  function counts(c: Counts): string {
    return t("export.counts", {
      connections: c.connections,
      credentials: c.credentials,
      folders: c.folders,
      macros: c.macros,
    });
  }

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

  async function doExport() {
    const r = await run(() =>
      api.exportBundle(
        exportScope,
        exportScope === "folder" ? exportFolder || null : null,
        exportPass,
        exportConfirm,
      ),
    );
    if (r) {
      exportResult = r;
      exportPass = "";
      exportConfirm = "";
    }
  }

  async function doPreview() {
    const p = await run(() => api.importPreview(importPass));
    if (p) {
      preview = p;
      importPass = "";
      importReport = null;
      strategies = {};
      for (const c of p.conflicts) strategies[`${c.kind}:${c.id}`] = defaultStrategy;
    }
  }

  async function doApply() {
    const p = preview;
    if (!p) return;
    const overrides = Object.entries(strategies).map(([key, strategy]) => ({ key, strategy }));
    const r = await run(() => api.importApply(p.preview_id, defaultStrategy, overrides));
    if (r) {
      importReport = r;
      preview = null;
    }
  }
</script>

<section class="card">
  <h2>{t("export.title")}</h2>
  <p class="muted">{t("export.hint")}</p>
  <label for="ex-scope">{t("export.scope")}</label>
  <select id="ex-scope" bind:value={exportScope}>
    <option value="all">{t("export.scope.all")}</option>
    <option value="folder">{t("export.scope.folder")}</option>
    <option value="no-secrets">{t("export.scope.noSecrets")}</option>
  </select>
  {#if exportScope === "folder"}
    <label for="ex-folder">{t("editor.folder")}</label>
    <select id="ex-folder" bind:value={exportFolder}>
      <option value="">{t("export.choose")}</option>
      {#each tree.folders as f (f.id)}
        <option value={f.id}>{f.name}</option>
      {/each}
    </select>
  {/if}
  <label for="ex-pass">{t("export.passphrase")}</label>
  <input id="ex-pass" type="password" bind:value={exportPass} autocomplete="new-password" />
  <label for="ex-confirm">{t("export.confirm")}</label>
  <input id="ex-confirm" type="password" bind:value={exportConfirm} autocomplete="new-password" />
  <button
    type="button"
    disabled={busy || exportPass.length < 8 || exportPass !== exportConfirm || (exportScope === "folder" && !exportFolder)}
    onclick={() => void doExport()}
  >
    {t("export.button")}
  </button>
  {#if exportResult}
    <p class="ok">
      <code>{exportResult.path}</code> —
      {counts(exportResult.counts)}.
    </p>
  {/if}
</section>

<section class="card">
  <h2>{t("export.importTitle")}</h2>
  <label for="im-pass">{t("export.importPassphrase")}</label>
  <input id="im-pass" type="password" bind:value={importPass} autocomplete="off" />
  <button type="button" disabled={busy || importPass.length === 0} onclick={() => void doPreview()}>
    {t("export.preview")}
  </button>
  {#if preview}
    <p>
      {counts(preview.counts)} ·
      {t("export.previewSecrets", { value: preview.includes_secrets ? t("export.yes") : t("export.no") })}
    </p>
    {#if preview.conflicts.length > 0}
      <label for="im-strategy">{tn("export.strategyDefault", preview.conflicts.length)}</label>
      <select id="im-strategy" bind:value={defaultStrategy}>
        <option value="keep">{t("export.strategy.keep")}</option>
        <option value="replace">{t("export.strategy.replace")}</option>
        <option value="duplicate">{t("export.strategy.duplicate")}</option>
      </select>
      <table>
        <thead>
          <tr><th>{t("export.colType")}</th><th>{t("export.colName")}</th><th>{t("export.colStrategy")}</th></tr>
        </thead>
        <tbody>
          {#each preview.conflicts as c (c.kind + c.id)}
            <tr>
              <td>{c.kind}</td>
              <td>{c.name}</td>
              <td>
                <select bind:value={strategies[`${c.kind}:${c.id}`]}>
                  <option value="keep">{t("export.row.keep")}</option>
                  <option value="replace">{t("export.row.replace")}</option>
                  <option value="duplicate">{t("export.row.duplicate")}</option>
                </select>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <p class="muted">{t("export.noConflict")}</p>
    {/if}
    <button type="button" disabled={busy} onclick={() => void doApply()}>{t("export.apply")}</button>
  {/if}
  {#if importReport}
    <p class="ok">
      {t("export.done", { counts: counts(importReport) })}
    </p>
  {/if}
</section>

<ExternalImport onrefresh={() => onrefresh?.()} {onerror} />
