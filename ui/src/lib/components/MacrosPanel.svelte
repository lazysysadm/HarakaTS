<script lang="ts">
  import { onMount } from "svelte";
  import { api, macroApi, message, type MacroMeta, type MacroPreview, type SessionInfo, type TreeDto } from "../api";
  import { t } from "../i18n.svelte";
  import { notify } from "../toast.svelte";

  interface Props {
    tree: TreeDto;
    onrefresh: (tree: TreeDto) => void;
    onerror: (msg: string) => void;
  }
  let { tree, onrefresh, onerror }: Props = $props();

  // Édition : `editId` = null (liste), "new" ou l'id de la macro.
  let editId = $state<string | null>(null);
  let name = $state("");
  let content = $state("");
  let confirmFlag = $state(false);
  // Contenu chargé (Afficher / modifier) ou saisi : sinon masqué et inchangé.
  let contentLoaded = $state(false);
  let masked = $state(true);
  let busy = $state(false);

  let sessions = $state<SessionInfo[]>([]);
  let target = $state("");
  let pendingSend = $state<{ macro: MacroMeta; preview: MacroPreview } | null>(null);
  let deleting = $state<MacroMeta | null>(null);
  let info = $state<string | null>(null);

  const connected = $derived(sessions.filter((s) => s.state === "connected"));

  onMount(() => {
    void loadSessions();
  });

  async function loadSessions() {
    try {
      sessions = await api.sessionList();
      if (!connected.some((s) => s.id === target)) target = connected[0]?.id ?? "";
    } catch (e) {
      onerror(message(e));
    }
  }

  function startNew() {
    editId = "new";
    name = "";
    content = "";
    confirmFlag = false;
    contentLoaded = true;
    masked = true;
    info = null;
  }

  function startEdit(m: MacroMeta) {
    editId = m.id;
    name = m.name;
    content = "";
    confirmFlag = m.confirm_before_send;
    contentLoaded = false;
    masked = true;
    info = null;
  }

  async function reveal() {
    if (!editId || editId === "new") return;
    try {
      content = await macroApi.reveal(editId);
      contentLoaded = true;
      masked = false;
    } catch (e) {
      onerror(message(e));
    }
  }

  async function save() {
    if (!name.trim()) {
      onerror(t("macro.nameRequired"));
      return;
    }
    if (editId === "new" && !content) {
      onerror(t("macro.contentRequired"));
      return;
    }
    busy = true;
    try {
      const id = editId === "new" ? null : editId;
      onrefresh(await api.macroUpsert(id, name.trim(), contentLoaded ? content : null, confirmFlag));
      cancelEdit();
    } catch (e) {
      onerror(message(e));
    } finally {
      busy = false;
    }
  }

  function cancelEdit() {
    editId = null;
    content = "";
    contentLoaded = false;
  }

  async function remove(m: MacroMeta) {
    busy = true;
    try {
      onrefresh(await api.macroDelete(m.id));
      deleting = null;
    } catch (e) {
      onerror(message(e));
    } finally {
      busy = false;
    }
  }

  async function send(m: MacroMeta) {
    info = null;
    if (!target) return;
    try {
      if (m.confirm_before_send) {
        pendingSend = { macro: m, preview: await macroApi.preview(m.id) };
        return;
      }
      await macroApi.send(m.id, target, false);
      info = t("macro.sent");
      notify(t("toast.done"));
    } catch (e) {
      onerror(message(e));
    }
  }

  async function confirmSend() {
    if (!pendingSend) return;
    const m = pendingSend.macro;
    pendingSend = null;
    try {
      await macroApi.send(m.id, target, true);
      info = t("macro.sent");
      notify(t("toast.done"));
    } catch (e) {
      onerror(message(e));
    }
  }
</script>

<div class="macros">
  {#if editId === null}
    <p class="muted">{t("macro.intro")}</p>
    <div class="macro-bar">
      <button type="button" onclick={startNew}>+ {t("macro.new")}</button>
      <span class="spacer"></span>
      <label class="inline" for="macro-target">{t("macro.sendTo")}</label>
      <select id="macro-target" bind:value={target} onfocus={() => void loadSessions()} disabled={connected.length === 0}>
        {#if connected.length === 0}<option value="">{t("macro.noSession")}</option>{/if}
        {#each connected as s (s.id)}<option value={s.id}>{s.title}</option>{/each}
      </select>
    </div>

    {#if tree.macros.length === 0}
      <p class="muted">{t("macro.empty")}</p>
    {:else}
      <table>
        <thead>
          <tr><th>{t("macro.name")}</th><th></th></tr>
        </thead>
        <tbody>
          {#each tree.macros as m (m.id)}
            <tr>
              <td>
                {m.name}
                {#if m.confirm_before_send}<span class="badge warn">{t("macro.confirmBadge")}</span>{/if}
              </td>
              <td class="macro-actions">
                <button type="button" disabled={!target || busy} onclick={() => void send(m)}>{t("macro.send")}</button>
                <button type="button" class="secondary" onclick={() => startEdit(m)}>{t("macro.edit")}</button>
                <button type="button" class="danger" onclick={() => (deleting = m)}>{t("macro.delete")}</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}

    {#if info}<p class="muted">{info}</p>{/if}

    {#if pendingSend}
      <div class="confirm-bar macro-confirm" role="alert">
        <div class="macro-confirm-text">
          <strong>{t("macro.sendConfirmTitle", { name: pendingSend.macro.name })}</strong>
          <div class="muted">{t("macro.sendConfirmInfo", { lines: pendingSend.preview.lines, bytes: pendingSend.preview.bytes })}</div>
          <pre class="macro-preview">{pendingSend.preview.preview}</pre>
        </div>
        <button type="button" class="danger" onclick={() => void confirmSend()}>{t("macro.send")}</button>
        <button type="button" class="secondary" onclick={() => (pendingSend = null)}>{t("common.cancel")}</button>
      </div>
    {/if}

    {#if deleting}
      <div class="confirm-bar" role="alert">
        <span>{t("macro.deleteConfirm", { name: deleting.name })}</span>
        <button type="button" class="danger" disabled={busy} onclick={() => deleting && void remove(deleting)}>{t("macro.delete")}</button>
        <button type="button" class="secondary" onclick={() => (deleting = null)}>{t("common.cancel")}</button>
      </div>
    {/if}
  {:else}
    <label for="macro-name">{t("macro.name")}</label>
    <input id="macro-name" type="text" bind:value={name} />

    <label for="macro-content">{t("macro.content")}</label>
    {#if contentLoaded}
      <textarea id="macro-content" class="macro-content" class:masked rows="8" bind:value={content} spellcheck="false"></textarea>
      <label class="inline"><input type="checkbox" checked={!masked} onchange={() => (masked = !masked)} /> {t("macro.showContent")}</label>
    {:else}
      <textarea id="macro-content" class="macro-content masked" rows="3" disabled placeholder={t("macro.contentMasked")}></textarea>
      <p><button type="button" class="secondary" onclick={() => void reveal()}>{t("macro.showEdit")}</button></p>
    {/if}
    <p class="muted">{t("macro.endHint")}</p>

    <label class="inline"><input type="checkbox" bind:checked={confirmFlag} /> {t("macro.confirmFlag")}</label>

    <div class="macro-bar">
      <button type="button" disabled={busy} onclick={() => void save()}>{t("common.save")}</button>
      <button type="button" class="secondary" onclick={cancelEdit}>{t("common.cancel")}</button>
    </div>
  {/if}
</div>
