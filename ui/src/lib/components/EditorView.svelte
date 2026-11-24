<script lang="ts">
  import { t } from "../i18n.svelte";
  import { formatDateTime } from "../i18n.svelte";
  import { formatSize } from "../sftp";
  import { eolSeparator, type Eol } from "../edit";
  import {
    activate,
    cancelCompare,
    closeDoc,
    docDirty,
    editor,
    initEdit,
    openExternal,
    currentRev,
    reload,
    save,
    saveLocal,
    saveMerged,
    setCurrent,
    setEol,
    startCompare,
  } from "../edit.svelte";
  import CodeEditor, { dropEditorState } from "./CodeEditor.svelte";
  import MergeCompare from "./MergeCompare.svelte";

  void initEdit();

  const doc = $derived(editor.active ? (editor.docs[editor.active] ?? null) : null);
  let closing = $state<string | null>(null);
  let cursor = $state({ line: 1, col: 1 });
  let lang = $state<string | null>(null);
  let cm = $state<{ getText: () => string } | undefined>();

  // Position et langage propres au document affiché.
  $effect(() => {
    void editor.active;
    cursor = { line: 1, col: 1 };
    lang = null;
  });

  function requestClose(id: string) {
    if (docDirty(id)) closing = id;
    else void doClose(id);
  }

  async function doClose(id: string) {
    closing = null;
    dropEditorState(id);
    await closeDoc(id);
  }

  function flushText() {
    if (doc && cm && doc.current !== null) setCurrent(doc.info.id, cm.getText());
  }

  function doSave(overwrite = false) {
    if (!doc) return;
    flushText();
    void save(doc.info.id, overwrite);
  }

  function overwrite() {
    if (!doc) return;
    if (doc.info.external) void saveLocal(doc.info.id, true);
    else doSave(true);
  }

  function describeRemote(r: { exists: boolean; size: number; mtime: number | null }): string {
    if (!r.exists) return t("edit.conflictGone");
    return t("edit.conflictChanged", {
      size: formatSize(r.size),
      date: r.mtime === null ? "?" : formatDateTime(r.mtime),
    });
  }

  const eols: Eol[] = ["lf", "crlf"];
</script>

<div class="edit">
  {#if editor.order.length === 0}
    <p class="muted edit-empty">{t("edit.none")}</p>
  {:else}
    <div class="edit-tabs" role="tablist">
      {#each editor.order as id (id)}
        {@const d = editor.docs[id]}
        <div class="edit-tab" class:active={id === editor.active} role="presentation">
          <button type="button" role="tab" aria-selected={id === editor.active} class="edit-tab-name" title={d.info.remote_path} onclick={() => activate(id)}>
            {#if docDirty(id)}<span class="dirty-dot" title={t("edit.modified")}>●</span>{/if}
            {d.info.name}
          </button>
          <button type="button" class="icon-btn always" title={t("common.close")} onclick={() => requestClose(id)}>✕</button>
        </div>
      {/each}
    </div>

    {#if closing && editor.docs[closing]}
      <div class="confirm-bar">
        <span>{t("edit.closeDirty", { name: editor.docs[closing].info.name })}</span>
        <button type="button" class="danger" onclick={() => void doClose(closing!)}>{t("edit.discardClose")}</button>
        <button type="button" class="secondary" onclick={() => (closing = null)}>{t("common.cancel")}</button>
      </div>
    {/if}

    {#if doc}
      {@const id = doc.info.id}
      {@const dirty = docDirty(id)}
      <div class="edit-toolbar">
        {#if !doc.info.external && doc.current !== null}
          <button type="button" class="tb primary" disabled={doc.busy || !dirty} title={"Ctrl+S"} onclick={() => doSave()}>💾 {t("common.save")}</button>
        {/if}
        <button type="button" class="tb" disabled={doc.busy} title={t("edit.reloadTitle")} onclick={() => void reload(id)}>⟳ {t("edit.reload")}</button>
        <button type="button" class="tb" disabled={doc.busy} title={t("edit.externalTitle")} onclick={() => void openExternal(id)}>↗ {t("edit.external")}</button>
        <span class="spacer"></span>
        <button type="button" class="tb" onclick={() => requestClose(id)}>{t("common.close")}</button>
      </div>

      {#if doc.info.external}
        <div class="edit-banner info">{t("edit.externalBanner")}</div>
      {/if}

      {#if doc.conflict}
        <div class="confirm-bar edit-conflict" role="alert">
          <span>⚠ {describeRemote(doc.conflict)}</span>
          <button type="button" class="danger" disabled={doc.busy} onclick={overwrite}>{t("edit.overwrite")}</button>
          {#if doc.conflict.exists}
            <button type="button" class="secondary" disabled={doc.busy} onclick={() => void reload(id)}>{t("edit.reloadRemote")}</button>
            <button type="button" class="secondary" disabled={doc.busy || doc.compare !== null} onclick={() => { flushText(); void startCompare(id); }}>{t("edit.compare")}</button>
          {/if}
        </div>
      {/if}

      {#if doc.notice}
        <div class="edit-banner" class:err={doc.notice.kind === "error"}>{doc.notice.text}</div>
      {/if}

      <div class="edit-body">
        {#if doc.compare}
          {#key id + ":" + doc.rev}
            <MergeCompare
              remote={doc.compare.remote}
              local={doc.compare.local}
              name={doc.info.name}
              sep={eolSeparator(doc.baseEol)}
              busy={doc.busy}
              onsave={(m) => void saveMerged(id, m)}
              oncancel={() => cancelCompare(id)}
            />
          {/key}
        {:else if doc.current === null}
          <div class="edit-noinline">
            <p>{doc.info.utf8 ? t("edit.tooLarge") : t("edit.notUtf8")}</p>
            <button type="button" disabled={doc.busy} onclick={() => void openExternal(id)}>↗ {t("edit.external")}</button>
          </div>
        {:else}
          {#key id + ":" + doc.rev}
            <CodeEditor
              bind:this={cm}
              {id}
              rev={doc.rev}
              text={doc.current}
              name={doc.info.name}
              sep={eolSeparator(doc.baseEol)}
              readonly={doc.info.external}
              onchange={(txt, r) => {
                // Un envoi tardif de l'ancienne instance (détruite par « Recharger » ou un
                // rafraîchissement) ne doit pas écraser le contenu fraîchement chargé.
                if (currentRev(id) === r) setCurrent(id, txt);
              }}
              oncursor={(line, col) => (cursor = { line, col })}
              onlang={(n) => (lang = n)}
              onsave={() => doSave()}
            />
          {/key}
        {/if}
      </div>

      <div class="edit-status">
        <span class="edit-path" title={doc.info.remote_path}>{doc.info.remote_path}</span>
        <span>{formatSize(doc.info.size)}</span>
        {#if doc.current !== null && !doc.info.external}
          <label class="inline" title={doc.info.line_ending === "mixed" ? t("edit.eolMixed") : t("edit.eolTitle")}>
            {t("edit.eol")}
            <select value={doc.eol} onchange={(ev) => setEol(id, ev.currentTarget.value as Eol)}>
              {#each eols as e (e)}
                <option value={e}>{e.toUpperCase()}</option>
              {/each}
            </select>
          </label>
        {/if}
        <span>{lang ?? t("edit.plainText")}</span>
        <span class="spacer"></span>
        {#if doc.current !== null}
          <span>{t("edit.cursor", { line: cursor.line, col: cursor.col })}</span>
        {/if}
        <span class:dirty-text={dirty}>{doc.info.external ? t("edit.stateExternal") : dirty ? t("edit.modified") : t("edit.saved")}</span>
      </div>
    {/if}
  {/if}
</div>
