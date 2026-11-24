<script lang="ts">
  import { onMount } from "svelte";
  import { api, macroApi, message, multiExecApi, type MacroMeta, type MacroPreview, type SessionInfo } from "../api";
  import { multiExec } from "../multiexec.svelte";
  import { t } from "../i18n.svelte";

  interface Props {
    onerror: (msg: string) => void;
  }
  let { onerror }: Props = $props();

  let sessions = $state<SessionInfo[]>([]);
  let picked = $state<Record<string, boolean>>({});
  let confirming = $state(false);
  let busy = $state(false);
  let line = $state("");
  let sentInfo = $state<string | null>(null);

  const connected = $derived(sessions.filter((s) => s.state === "connected"));
  const selectedIds = $derived(connected.filter((s) => picked[s.id]).map((s) => s.id));
  const selected = $derived(connected.filter((s) => picked[s.id]));
  const members = $derived(sessions.filter((s) => multiExec.sessions.includes(s.id)));

  let macros = $state<MacroMeta[]>([]);
  let macroId = $state("");
  let pendingMacro = $state<{ macro: MacroMeta; preview: MacroPreview } | null>(null);

  // Une macro envoyée à un membre est diffusée à tous les membres (même chemin que la saisie).
  async function sendMacro(confirmed: boolean) {
    const m = macros.find((x) => x.id === macroId);
    const target = multiExec.sessions[0];
    if (!m || !target) return;
    try {
      if (m.confirm_before_send && !confirmed) {
        pendingMacro = { macro: m, preview: await macroApi.preview(m.id) };
        return;
      }
      await macroApi.send(m.id, target, confirmed);
      pendingMacro = null;
      sentInfo = t("multiexec.macroSent", { name: m.name, n: multiExec.sessions.length });
    } catch (e) {
      onerror(message(e));
    }
  }

  async function refresh() {
    try {
      macros = (await api.treeGet()).macros;
      sessions = await api.sessionList();
    } catch (e) {
      onerror(message(e));
    }
  }

  onMount(() => {
    void refresh();
  });

  async function setEnabled(enabled: boolean) {
    busy = true;
    try {
      await multiExecApi.set(enabled, enabled ? selectedIds : []);
      confirming = false;
      sentInfo = null;
    } catch (e) {
      onerror(message(e));
    } finally {
      busy = false;
    }
  }

  async function sendLine() {
    if (!line) return;
    try {
      const n = await multiExecApi.send(line);
      sentInfo = t("multiexec.sent", { n });
      line = "";
    } catch (e) {
      onerror(message(e));
    }
  }
</script>

<div class="mx-panel">
  <p>
    <span class="badge" class:warn={multiExec.enabled}>
      {multiExec.enabled ? t("multiexec.active", { n: multiExec.sessions.length }) : t("multiexec.inactive")}
    </span>
  </p>
  <p class="muted">{t("multiexec.intro")}</p>

  {#if multiExec.enabled}
    <h3>{t("multiexec.members")}</h3>
    <ul class="mx-list">
      {#each members as s (s.id)}
        <li>{s.title}</li>
      {/each}
    </ul>

    <label for="mx-line">{t("multiexec.sendLabel")}</label>
    <form
      class="mx-send"
      onsubmit={(ev) => {
        ev.preventDefault();
        void sendLine();
      }}
    >
      <input id="mx-line" type="text" bind:value={line} placeholder={t("multiexec.sendPlaceholder")} autocomplete="off" />
      <button type="submit" disabled={!line}>{t("multiexec.send")}</button>
    </form>
    {#if sentInfo}<p class="muted">{sentInfo}</p>{/if}

    <label for="mx-macro">{t("multiexec.macroLabel")}</label>
    {#if macros.length === 0}
      <p class="muted">{t("multiexec.macroNone")}</p>
    {:else}
      <div class="mx-send">
        <select id="mx-macro" bind:value={macroId} onchange={() => (pendingMacro = null)}>
          <option value="">{t("multiexec.macroPick")}</option>
          {#each macros as m (m.id)}
            <option value={m.id}>{m.name}{m.confirm_before_send ? " ⚠" : ""}</option>
          {/each}
        </select>
        <button type="button" disabled={!macroId} onclick={() => void sendMacro(false)}>{t("macro.send")}</button>
      </div>
    {/if}
    {#if pendingMacro}
      <div class="confirm-bar macro-confirm" role="alert">
        <div class="macro-confirm-text">
          <strong>{t("macro.sendConfirmTitle", { name: pendingMacro.macro.name })}</strong>
          <div class="muted">{t("macro.sendConfirmInfo", { lines: pendingMacro.preview.lines, bytes: pendingMacro.preview.bytes })}</div>
          <pre class="macro-preview">{pendingMacro.preview.preview}</pre>
        </div>
        <button type="button" class="danger" onclick={() => void sendMacro(true)}>{t("macro.send")}</button>
        <button type="button" class="secondary" onclick={() => (pendingMacro = null)}>{t("common.cancel")}</button>
      </div>
    {/if}

    <p>
      <button type="button" class="danger" disabled={busy} onclick={() => void setEnabled(false)}>{t("multiexec.disable")}</button>
    </p>
  {:else}
    <div class="mx-head">
      <h3>{t("multiexec.pick")}</h3>
      <span class="spacer"></span>
      <button type="button" class="secondary" onclick={() => void refresh()}>{t("multiexec.refresh")}</button>
    </div>
    {#if connected.length === 0}
      <p class="muted">{t("multiexec.none")}</p>
    {:else}
      <ul class="mx-list">
        {#each connected as s (s.id)}
          <li>
            <label class="inline">
              <input type="checkbox" bind:checked={picked[s.id]} disabled={confirming} />
              {s.title}
            </label>
          </li>
        {/each}
      </ul>
    {/if}

    {#if confirming}
      <div class="confirm-bar mx-confirm" role="alert">
        <div class="mx-confirm-text">
          <strong>{t("multiexec.confirmTitle")}</strong>
          <div>{t("multiexec.confirmText", { n: selected.length })}</div>
          <ul>
            {#each selected as s (s.id)}<li>{s.title}</li>{/each}
          </ul>
        </div>
        <button type="button" class="danger" disabled={busy} onclick={() => void setEnabled(true)}>{t("multiexec.confirmYes")}</button>
        <button type="button" class="secondary" onclick={() => (confirming = false)}>{t("common.cancel")}</button>
      </div>
    {:else}
      <p>
        <button type="button" disabled={selectedIds.length < 2} onclick={() => (confirming = true)}>{t("multiexec.enable")}</button>
        {#if selectedIds.length < 2}<span class="muted">{t("multiexec.min")}</span>{/if}
      </p>
    {/if}
  {/if}
</div>
