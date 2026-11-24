<script lang="ts">
  import { onMount } from "svelte";
  import { api, statusApi, message, type AgentStatus, type KnownHostDto, type LogFileDto, type Settings } from "../api";
  import { appState, uiPrefs } from "../state.svelte";
  import { availableLocales, formatDateTime, setLocale, t, tn } from "../i18n.svelte";
  import { notify } from "../toast.svelte";

  interface Props {
    onerror: (msg: string) => void;
  }
  let { onerror }: Props = $props();

  let settings = $state<Settings | null>(null);
  let autolock = $state(15);
  let clearSeconds = $state(30);
  let clipboardDefault = $state("bidirectional");
  let pasteConfirm = $state(true);
  let copyOnSelect = $state(false);
  let terminalInTabs = $state(false);
  let disconnectOnLock = $state(true);
  let serverStatus = $state(false);
  let serverStatusInterval = $state(60);
  let highlight = $state(true);
  let followTerminal = $state(false);
  let logEncrypt = $state(true);
  let logRetention = $state(30);
  let agent = $state<AgentStatus | null>(null);
  let logs = $state<LogFileDto[]>([]);
  let logView = $state<{ name: string; text: string } | null>(null);
  let logMessage = $state<string | null>(null);
  let parallelism = $state(3);
  let locale = $state("fr");
  let theme = $state("system");
  let fontSize = $state(11);
  let scrollback = $state(10000);
  let externalEditor = $state("");
  let kdfMaster = $state("");
  let busy = $state(false);
  let kdfDone = $state(false);
  let knownHosts = $state<KnownHostDto[]>([]);
  let khMessage = $state<string | null>(null);

  onMount(() => {
    void load();
    void loadKnownHosts();
    void loadLogs();
    void api.sshAgentStatus().then((a) => (agent = a)).catch(() => (agent = null));
  });

  async function loadKnownHosts() {
    try {
      knownHosts = await api.knownHostsList();
    } catch (e) {
      onerror(message(e));
    }
  }

  async function loadLogs() {
    try {
      logs = await api.sessionLogsList();
    } catch {
      logs = [];
    }
  }

  async function viewLog(name: string) {
    try {
      logView = { name, text: await api.sessionLogRead(name) };
    } catch (e) {
      logMessage = message(e);
    }
  }

  async function deleteLog(name: string) {
    if (!confirm(t("logs.deleteConfirm", { name }))) return;
    try {
      await api.sessionLogDelete(name);
      notify(t("toast.deleted"));
      if (logView?.name === name) logView = null;
      await loadLogs();
    } catch (e) {
      logMessage = message(e);
    }
  }

  async function purgeLogs() {
    try {
      const n = await api.sessionLogsPurge(null);
      logMessage = tn("logs.purged", n);
      await loadLogs();
    } catch (e) {
      logMessage = message(e);
    }
  }

  async function forgetKnownHost(kh: KnownHostDto) {
    if (!confirm(t("kh.forgetConfirm", { host: kh.host_field }))) return;
    try {
      const n = await api.knownHostsForget(kh.host_field, kh.fingerprint);
      khMessage = tn("kh.forgotten", n);
      await loadKnownHosts();
    } catch (e) {
      khMessage = message(e);
    }
  }

  async function clearKnownHosts() {
    if (!confirm(t("kh.clearConfirm"))) return;
    try {
      const n = await api.knownHostsClear();
      khMessage = tn("kh.cleared", n);
      notify(t("toast.deleted"));
      await loadKnownHosts();
    } catch (e) {
      onerror(message(e));
    }
  }

  async function importSystemKnownHosts() {
    try {
      const n = await api.knownHostsImportSystem();
      khMessage = tn("kh.imported", n);
      await loadKnownHosts();
    } catch (e) {
      onerror(message(e));
    }
  }

  async function load() {
    try {
      const s = await api.settingsGet();
      settings = s;
      autolock = s.autolock_minutes;
      clearSeconds = s.clipboard_clear_seconds;
      clipboardDefault = s.clipboard_default;
      pasteConfirm = s.paste_confirm_multiline;
      copyOnSelect = s.copy_on_select;
      terminalInTabs = s.terminal_in_tabs;
      disconnectOnLock = s.disconnect_on_lock;
      serverStatus = s.server_status;
      serverStatusInterval = s.server_status_interval_s;
      highlight = s.terminal_highlight;
      followTerminal = s.sftp_follow_terminal;
      logEncrypt = s.session_log_encrypt;
      logRetention = s.session_log_retention_days;
      parallelism = s.transfer_parallelism;
      locale = s.locale;
      theme = s.theme;
      fontSize = s.terminal_font_size;
      scrollback = s.terminal_scrollback;
      externalEditor = s.external_editor ?? "";
      setLocale(s.locale);
      uiPrefs.theme = s.theme;
    } catch (e) {
      onerror(message(e));
    }
  }

  async function save() {
    busy = true;
    try {
      settings = await api.settingsSet({
        autolock_minutes: autolock,
        clipboard_clear_seconds: clearSeconds,
        clipboard_default: clipboardDefault,
        paste_confirm_multiline: pasteConfirm,
        copy_on_select: copyOnSelect,
        terminal_in_tabs: terminalInTabs,
        disconnect_on_lock: disconnectOnLock,
        server_status: serverStatus,
        server_status_interval_s: Math.min(3600, Math.max(15, Math.round(serverStatusInterval) || 60)),
        terminal_highlight: highlight,
        sftp_follow_terminal: followTerminal,
        session_log_encrypt: logEncrypt,
        session_log_retention_days: logRetention,
        transfer_parallelism: parallelism,
        locale,
        theme,
        terminal_font_size: fontSize,
        terminal_scrollback: scrollback,
        external_editor: externalEditor.trim() === "" ? null : externalEditor.trim(),
      });
      setLocale(locale);
      uiPrefs.theme = theme;
      void statusApi.refresh().catch(() => undefined);
      notify(t("toast.saved"));
    } catch (e) {
      onerror(message(e));
    } finally {
      busy = false;
    }
  }

  async function toggleKeyring() {
    try {
      appState.status = await api.keyringSetEnabled(!appState.status?.keyring_cache_enabled);
      notify(t("toast.saved"));
    } catch (e) {
      onerror(message(e));
    }
  }

  async function recalibrate() {
    if (!kdfMaster) return;
    busy = true;
    kdfDone = false;
    try {
      await api.recalibrateKdf(kdfMaster);
      kdfMaster = "";
      kdfDone = true;
      notify(t("toast.done"));
    } catch (e) {
      onerror(message(e));
    } finally {
      busy = false;
    }
  }
</script>

{#snippet tip(text: string)}
  <span class="tip" title={text} aria-label={text} role="img">ⓘ</span>
{/snippet}

<section class="card">
  <h2>{t("settings.title")}</h2>
  <div class="form-grid">
    <div>
      <label for="st-autolock">{t("settings.autolock")} {@render tip(t("settings.tip.autolock"))}</label>
      <input id="st-autolock" type="number" min="0" max="1440" bind:value={autolock} />
    </div>
    <div>
      <label for="st-clear">{t("settings.clipboardTtl")} {@render tip(t("settings.tip.clipboardTtl"))}</label>
      <input id="st-clear" type="number" min="0" max="600" bind:value={clearSeconds} />
    </div>
    <div>
      <label for="st-clip">{t("settings.clipboardDefault")} {@render tip(t("settings.tip.clipboardDefault"))}</label>
      <select id="st-clip" bind:value={clipboardDefault}>
        <option value="disabled">{t("settings.clip.disabled")}</option>
        <option value="l2r">{t("settings.clip.l2r")}</option>
        <option value="r2l">{t("settings.clip.r2l")}</option>
        <option value="bidirectional">{t("settings.clip.bidirectional")}</option>
      </select>
    </div>
    <div>
      <label for="st-par">{t("settings.parallelism")} {@render tip(t("settings.tip.parallelism"))}</label>
      <input id="st-par" type="number" min="1" max="8" bind:value={parallelism} />
    </div>
    <div>
      <label for="st-locale">{t("settings.locale")} {@render tip(t("settings.tip.locale"))}</label>
      <select id="st-locale" bind:value={locale}>
        {#each availableLocales() as l (l.code)}
          <option value={l.code}>{l.name}</option>
        {/each}
      </select>
    </div>
    <div>
      <label for="st-theme">{t("settings.theme")} {@render tip(t("settings.tip.theme"))}</label>
      <select id="st-theme" bind:value={theme}>
        <option value="system">{t("theme.system")}</option>
        <option value="gris">{t("theme.gris")}</option>
        <option value="bleu">{t("theme.bleu")}</option>
        <option value="clair">{t("theme.clair")}</option>
      </select>
    </div>
    <div>
      <label for="st-fontsize">{t("settings.fontSize")} {@render tip(t("settings.tip.fontSize"))}</label>
      <input id="st-fontsize" type="number" min="8" max="32" bind:value={fontSize} />
    </div>
    <div>
      <label for="st-scrollback">{t("settings.scrollback")} {@render tip(t("settings.tip.scrollback"))}</label>
      <input id="st-scrollback" type="number" min="0" max="1000000" bind:value={scrollback} />
    </div>
    <div class="wide-field">
      <label for="st-editor">{t("edit.setting")}</label>
      <input id="st-editor" type="text" placeholder={"code --wait {file}"} bind:value={externalEditor} />
      <p class="muted small">{t("edit.settingHint")}</p>
    </div>
  </div>
  <label class="inline" title={t("settings.tip.pasteConfirm")}><input type="checkbox" bind:checked={pasteConfirm} /> {t("settings.pasteConfirm")} {@render tip(t("settings.tip.pasteConfirm"))}</label>
  <label class="inline" title={t("settings.tip.copyOnSelect")}><input type="checkbox" bind:checked={copyOnSelect} /> {t("settings.copyOnSelect")} {@render tip(t("settings.tip.copyOnSelect"))}</label>
  <label class="inline" title={t("settings.terminalHighlightHelp")}><input type="checkbox" bind:checked={highlight} /> {t("settings.terminalHighlight")} {@render tip(t("settings.terminalHighlightHelp"))}</label>
  <p class="muted small">{t("settings.terminalHighlightHelp")}</p>
  <label class="inline" title={t("settings.disconnectOnLockHelp")}><input type="checkbox" bind:checked={disconnectOnLock} /> {t("settings.disconnectOnLock")} {@render tip(t("settings.disconnectOnLockHelp"))}</label>
  <p class="muted small">{t("settings.disconnectOnLockHelp")}</p>
  <label class="inline" title={t("settings.tip.serverStatus")}><input type="checkbox" bind:checked={serverStatus} /> {t("settings.serverStatus")} {@render tip(t("settings.tip.serverStatus"))}</label>
  {#if serverStatus}
    <label class="row" title={t("settings.tip.serverStatusInterval")}>
      {t("settings.serverStatusInterval")}
      <input type="number" min="15" max="3600" bind:value={serverStatusInterval} />
    </label>
  {/if}
  <label class="inline" title={t("settings.terminalTabsHelp")}><input type="checkbox" bind:checked={terminalInTabs} /> {t("settings.terminalTabs")} {@render tip(t("settings.terminalTabsHelp"))}</label>
  <p class="muted small">{t("settings.terminalTabsHelp")}</p>
  <label class="inline" title={t("settings.sftpFollowTitle")}>
    <input type="checkbox" bind:checked={followTerminal} /> {t("settings.sftpFollow")} {@render tip(t("settings.sftpFollowTitle"))}
  </label>
  <label class="inline" title={t("logs.encryptHint")}>
    <input type="checkbox" bind:checked={logEncrypt} /> {t("logs.encrypt")} {@render tip(t("logs.encryptHint"))}
  </label>
  <label class="row" title={t("settings.tip.logRetention")}>
    {t("logs.retention")} {@render tip(t("settings.tip.logRetention"))}
    <input type="number" min="0" max="3650" bind:value={logRetention} />
  </label>
  <label class="row" title={t("settings.tip.keyring")}>
    <input
      type="checkbox"
      checked={appState.status?.keyring_cache_enabled ?? false}
      disabled={!appState.status?.keyring_available || busy}
      onchange={() => void toggleKeyring()}
    />
    {t("settings.keyring")} {appState.status?.keyring_available ? t("settings.keyringAvailable") : t("settings.keyringUnavailable")} {@render tip(t("settings.tip.keyring"))}
  </label>
  <button type="button" disabled={busy} onclick={() => void save()}>{t("common.save")}</button>
  {#if settings}
    <p class="muted">
      {t("settings.suspendLock", { value: settings.lock_on_suspend ? t("settings.yes") : t("settings.no") })} ·
      {t("settings.sessionLock", { value: settings.lock_on_session_lock ? t("settings.yes") : t("settings.no") })}
    </p>
  {/if}

  <h3>{t("settings.kdf")} {@render tip(t("settings.tip.kdf"))}</h3>
  <p class="muted">{t("settings.recalibrate")}</p>
  <label for="st-kdf">{t("settings.masterPassword")}</label>
  <input id="st-kdf" type="password" bind:value={kdfMaster} autocomplete="current-password" />
  <button type="button" disabled={busy || kdfMaster.length === 0} onclick={() => void recalibrate()}>
    {t("settings.recalibrate")}
  </button>
  {#if kdfDone}<p class="ok">OK</p>{/if}

  <h3>{t("agent.title")}</h3>
  <p class="muted">{t("agent.hint")}</p>
  {#if agent === null}
    <p class="muted">…</p>
  {:else if !agent.available}
    <p class="muted">{t("agent.unavailable")}</p>
  {:else if agent.keys.length === 0}
    <p class="muted">{t("agent.empty")}</p>
  {:else}
    <table>
      <tbody>
        {#each agent.keys as k (k.fingerprint)}
          <tr>
            <td>{k.hardware ? "🔑 " : ""}{k.algorithm}{k.certificate ? " (cert)" : ""}</td>
            <td><code>{k.fingerprint}</code></td>
            <td>{k.comment}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  <h3>{t("logs.title")}</h3>
  <p class="muted">{t("logs.hint")}</p>
  <button type="button" class="secondary" onclick={() => void loadLogs()}>{t("kh.refresh")}</button>
  <button type="button" class="secondary" title={t("settings.tip.purgeLogs")} onclick={() => void purgeLogs()}>{t("logs.purgeNow")}</button>
  {#if logMessage}<p class="ok">{logMessage}</p>{/if}
  {#if logs.length > 0}
    <table>
      <thead>
        <tr><th>{t("logs.colName")}</th><th>{t("unlock.date")}</th><th>{t("logs.colSize")}</th><th></th></tr>
      </thead>
      <tbody>
        {#each logs as l (l.name)}
          <tr>
            <td>{l.encrypted ? "🔒 " : ""}{l.name}</td>
            <td>{l.modified_epoch ? formatDateTime(l.modified_epoch) : "—"}</td>
            <td>{Math.max(1, Math.round(l.size / 1024))} Ko</td>
            <td>
              <button type="button" class="secondary" onclick={() => void viewLog(l.name)}>{t("logs.view")}</button>
              <button type="button" class="secondary" onclick={() => void deleteLog(l.name)}>{t("logs.delete")}</button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <p class="muted">{t("logs.none")}</p>
  {/if}
  {#if logView}
    <h4>{logView.name}</h4>
    <pre class="logview">{logView.text}</pre>
    <button type="button" class="secondary" onclick={() => (logView = null)}>{t("common.close")}</button>
  {/if}

  <h3>{t("kh.title")}</h3>
  <p class="muted">{t("kh.hint")}</p>
  <button type="button" class="secondary" title={t("settings.tip.importKnownHosts")} onclick={() => void importSystemKnownHosts()}>
    {t("kh.importSystem")}
  </button>
  <button type="button" class="secondary" onclick={() => void loadKnownHosts()}>{t("kh.refresh")}</button>
  <button type="button" class="secondary" disabled={knownHosts.length === 0} onclick={() => void clearKnownHosts()}>{t("kh.clearAll")}</button>
  {#if khMessage}<p class="ok">{khMessage}</p>{/if}
  {#if knownHosts.length > 0}
    <table>
      <thead>
        <tr><th>{t("kh.colHost")}</th><th>{t("kh.colType")}</th><th>{t("kh.colFingerprint")}</th><th></th></tr>
      </thead>
      <tbody>
        {#each knownHosts as kh, i (i)}
          <tr>
            <td>{kh.host_field}</td>
            <td>{kh.key_type}</td>
            <td><code>{kh.fingerprint}</code></td>
            <td><button type="button" class="secondary" onclick={() => void forgetKnownHost(kh)}>{t("kh.forget")}</button></td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <p class="muted">{t("kh.none")}</p>
  {/if}
</section>

<style>
  .tip {
    cursor: help;
    opacity: 0.65;
    margin-left: 4px;
    font-size: 12px;
  }
  .tip:hover {
    opacity: 1;
  }
  .logview {
    max-height: 320px;
    overflow: auto;
    white-space: pre-wrap;
    font-size: 12px;
    padding: 8px;
    border: 1px solid var(--border, #8884);
    border-radius: 6px;
  }
</style>
