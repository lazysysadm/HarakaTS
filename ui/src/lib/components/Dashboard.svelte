<script lang="ts">
  import { onMount } from "svelte";
  import { api, message, multiExecApi, type Connection, type TreeDto } from "../api";
  import { appState, uiPrefs } from "../state.svelte";
  import { setLocale, t } from "../i18n.svelte";
  import Tree from "./Tree.svelte";
  import ConnectionEditor from "./ConnectionEditor.svelte";
  import CredentialManager from "./CredentialManager.svelte";
  import Palette from "./Palette.svelte";
  import SettingsPanel from "./SettingsPanel.svelte";
  import ExportImport from "./ExportImport.svelte";
  import AuditView from "./AuditView.svelte";
  import SftpPanel from "./SftpPanel.svelte";
  import type { Component } from "svelte";
  import About from "./About.svelte";
  import { activeTransferCount, initTransfers } from "../sftp.svelte";
  import { sftpApi } from "../api";
  import { docCount, editor } from "../edit.svelte";
  import Modal from "./Modal.svelte";
  import FloatingPanel from "./FloatingPanel.svelte";
  import MultiExecPanel from "./MultiExecPanel.svelte";
  import TunnelsPanel from "./TunnelsPanel.svelte";
  import { initTunnels, liveTunnelCount } from "../tunnels.svelte";
  import MacrosPanel from "./MacrosPanel.svelte";
  import { initMultiExec, multiExec } from "../multiexec.svelte";
  import { activateSession, initSessions, refreshSessions, sessions } from "../sessions.svelte";
  import { initStatus, serverStatus } from "../status.svelte";
  import { notify } from "../toast.svelte";

  type ModalKind = "editor" | "credentials" | "settings" | "transfer" | "audit" | "about" | "macros" | null;

  let tree = $state<TreeDto | null>(null);
  let selected = $state<string | null>(null);
  let modal = $state<ModalKind>(null);
  let editorConnection = $state<Connection | null>(null);
  let editorFolder = $state<string | null>(null);
  let paletteOpen = $state(false);
  // Multi-exec : panneau flottant non modal (l'interface reste utilisable).
  let mxOpen = $state(false);
  let menuOpen = $state(false);
  function pick(action: () => void) {
    menuOpen = false;
    action();
  }
  // Vue SFTP intégrée (non modale) : l'arbre et la barre d'outils restent utilisables.
  let sftpMounted = $state(false);
  let sftpVisible = $state(false);
  // Éditeur distant (CodeMirror, chargé à la demande) : vue exclusive avec SFTP.
  let editVisible = $state(false);
  // Gestionnaire de tunnels (M7) : vue exclusive avec SFTP et l'éditeur.
  let tunMounted = $state(false);
  let tunVisible = $state(false);
  // Mode onglets : le tableau de bord est une colonne étroite à gauche ; elle s'élargit quand une
  // vue large (SFTP/éditeur/tunnels, fenêtre modale, multi-exec) est ouverte.
  let tabsMode = $state(false);
  const wide = $derived(modal !== null || sftpVisible || editVisible || tunVisible || mxOpen);
  $effect(() => {
    const w = wide;
    if (tabsMode) void api.layoutWide(w).catch(() => undefined);
  });
  // Les réglages ont pu changer l'option : relecture à la fermeture d'une fenêtre.
  $effect(() => {
    if (modal === null) {
      void api
        .settingsGet()
        .then((s) => {
          tabsMode = s.terminal_in_tabs;
          serverStatus.enabled = s.server_status;
        })
        .catch(() => undefined);
    }
  });
  let EditorViewC = $state<Component | null>(null);
  let lastShowSeq = 0;

  onMount(() => {
    void loadTree();
    void initTransfers();
    void initMultiExec();
    void initTunnels();
    void initSessions();
    void initStatus();
    const unsubs: Array<() => void> = [];
    void sftpApi.onOpen((session) => openSftp(session)).then((u) => unsubs.push(u));
    const onKey = (ev: KeyboardEvent) => {
      if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "k") {
        ev.preventDefault();
        paletteOpen = !paletteOpen;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      for (const u of unsubs) u();
    };
  });

  async function loadTree() {
    try {
      tree = await api.treeGet();
      const s = await api.settingsGet();
      tabsMode = s.terminal_in_tabs;
      serverStatus.enabled = s.server_status;
      setLocale(s.locale);
      uiPrefs.theme = s.theme;
    } catch (e) {
      appState.error = message(e);
    }
  }

  /** Arbre mis à jour après une action (création, modification, suppression, déplacement). */
  function refresh(next: TreeDto) {
    tree = next;
    notify(t("toast.saved"));
  }

  function err(msg: string) {
    appState.error = msg;
  }

  function editConnection(id: string | null, folderId: string | null = null) {
    selected = id;
    editorConnection = tree?.connections.find((c) => c.id === id) ?? null;
    editorFolder = folderId;
    modal = "editor";
  }

  async function startSession(connectionId: string) {
    if (!tree?.connections.some((c) => c.id === connectionId)) return;
    modal = null;
    try {
      // Terminal natif GTK+VTE (seul moteur, comme Remmina) : une fenêtre par session.
      await api.sessionOpenVte(connectionId);
    } catch (e) {
      err(message(e));
    }
  }

  function openSftp(session: string | null) {
    if (session) void activateSession(session, false);
    sftpMounted = true;
    sftpVisible = true;
    editVisible = false;
    tunVisible = false;
    modal = null;
  }

  function showEditor() {
    sftpVisible = false;
    tunVisible = false;
    editVisible = true;
    modal = null;
    if (!EditorViewC) void import("./EditorView.svelte").then((m) => (EditorViewC = m.default as unknown as Component));
  }

  function toggleEditor() {
    if (editVisible) editVisible = false;
    else showEditor();
  }

  // Un document vient d'être ouvert (panneau SFTP…) : afficher l'éditeur.
  $effect(() => {
    const seq = editor.showSeq;
    if (seq !== lastShowSeq) {
      lastShowSeq = seq;
      showEditor();
    }
  });

  // Nouvelle session ouverte : le panneau SFTP s'ouvre dessus automatiquement
  // (sans voler le focus à la fenêtre terminal) et s'y connecte dès qu'elle est prête.
  let lastOpenedSeq = 0;
  $effect(() => {
    const seq = sessions.openedSeq;
    if (seq !== lastOpenedSeq) {
      lastOpenedSeq = seq;
      // Mode onglets : le SFTP s'ouvre à la demande (bouton « Fichiers (SFTP) » du terminal).
      void api
        .settingsGet()
        .then((s) => {
          if (s.terminal_in_tabs) return;
          sftpMounted = true;
          sftpVisible = true;
          editVisible = false;
          tunVisible = false;
        })
        .catch(() => {
          // réglages indisponibles : pas d'ouverture automatique
        });
    }
  });

  function toggleTunnels() {
    if (tunVisible) {
      tunVisible = false;
      return;
    }
    tunMounted = true;
    tunVisible = true;
    sftpVisible = false;
    editVisible = false;
    modal = null;
  }

  function toggleSftp() {
    if (sftpVisible) sftpVisible = false;
    else openSftp(null);
  }

  async function closeSession(id: string) {
    try {
      await api.sessionClose(id);
      await refreshSessions();
    } catch (e) {
      err(message(e));
    }
  }

  async function lock() {
    try {
      appState.status = await api.lock();
    } catch (e) {
      err(message(e));
    }
  }

  function paletteAction(action: string) {
    if (action === "lock") void lock();
    else if (action === "sftp") openSftp(null);
    else if (action === "settings" || action === "transfer" || action === "audit" || action === "credentials") {
      modal = action;
    }
  }
</script>

<div class="app-shell" class:tabs-mode={tabsMode} class:view-open={sftpVisible || editVisible || tunVisible}>
  <header class="toolbar card">
    <div class="menu-wrap">
      <button type="button" class="secondary" class:active-view={menuOpen} aria-haspopup="menu" aria-expanded={menuOpen} onclick={() => (menuOpen = !menuOpen)}>☰ {t("dash.menu")}{#if multiExec.enabled} ●{/if}</button>
      {#if menuOpen}
        <div class="menu-backdrop" role="presentation" onclick={() => (menuOpen = false)}></div>
        <div class="menu-panel card" role="menu">
          <button type="button" role="menuitem" class:active-view={sftpVisible} onclick={() => pick(toggleSftp)}>📁 {t("tabs.sftp")}{#if activeTransferCount() > 0} ({activeTransferCount()}){/if}</button>
          <button type="button" role="menuitem" class:active-view={editVisible} onclick={() => pick(toggleEditor)}>📝 {t("edit.tab")}{#if docCount() > 0} ({docCount()}){/if}</button>
          <button type="button" role="menuitem" class:active-view={tunVisible} onclick={() => pick(toggleTunnels)}>🔀 {t("tunnel.tab")} ({liveTunnelCount()})</button>
          <button type="button" role="menuitem" class:active-view={multiExec.enabled} onclick={() => pick(() => (mxOpen = !mxOpen))}>{t("multiexec.button")}{#if multiExec.enabled} ({multiExec.sessions.length}){/if}</button>
          <button type="button" role="menuitem" onclick={() => pick(() => (modal = "macros"))}>{t("macro.button")}</button>
          <hr />
          <button type="button" role="menuitem" onclick={() => pick(() => (modal = "credentials"))}>{t("tabs.credentials")}</button>
          <button type="button" role="menuitem" onclick={() => pick(() => (modal = "transfer"))}>{t("tabs.transfer")}</button>
          <button type="button" role="menuitem" onclick={() => pick(() => (modal = "audit"))}>{t("tabs.audit")}</button>
          <hr />
          <button type="button" role="menuitem" onclick={() => pick(() => (modal = "settings"))}>⚙ {t("tabs.settings")}</button>
          <button type="button" role="menuitem" onclick={() => pick(() => (paletteOpen = true))}>{t("dash.palette")} <kbd>Ctrl+K</kbd></button>
          <button type="button" role="menuitem" onclick={() => pick(() => (modal = "about"))}>{t("about.button")}</button>
        </div>
      {/if}
    </div>
    <span class="spacer"></span>
    <button type="button" class="danger" title={t("dash.lock")} onclick={() => void lock()}>🔒</button>
  </header>

  {#if sessions.list.length > 0 && !tabsMode}
    <nav class="sessbar" aria-label={t("tabs.sessions")}>
      {#each sessions.list as s (s.id)}
        <span class="sess-chip" class:active={s.id === sessions.active}>
          <button type="button" class="sess-main" title={`${t("sessbar.focus")} — ${s.state}`} onclick={() => void activateSession(s.id)}>
            <span class="dot" class:ok={s.state === "connected"} class:bad={s.state === "failed" || s.state === "closed"}></span>{s.title}
          </button>
          <button type="button" class="sess-x" title={t("sessbar.close")} aria-label={t("sessbar.close")} onclick={() => void closeSession(s.id)}>✕</button>
        </span>
      {/each}
    </nav>
  {/if}

  {#if multiExec.enabled}
    <div class="mx-banner" role="alert">
      <span>⚠ {t("multiexec.banner", { n: multiExec.sessions.length })}</span>
      <span class="spacer"></span>
      <button type="button" onclick={() => void multiExecApi.set(false, []).catch((e) => err(message(e)))}>{t("multiexec.disable")}</button>
    </div>
  {/if}

  <div class="layout">
    <aside class="sidebar">
      {#if tree}
        <Tree
          {tree}
          {selected}
          onselect={(id) => (selected = id)}
          onconnect={(id) => void startSession(id)}
          onedit={(id) => editConnection(id)}
          onrefresh={refresh}
          onerror={err}
          onnewconnection={(folderId) => editConnection(null, folderId)}
        />
      {:else}
        <p class="muted">{t("common.loading")}</p>
      {/if}
    </aside>

    <main class="content">
      {#if sftpMounted}
        <div class="sftp-view" hidden={!sftpVisible}>
          <div class="sftp-view-head">
            <strong>📁 {t("tabs.sftp")}</strong>
            <span class="spacer"></span>
            <button type="button" class="secondary" onclick={() => (sftpVisible = false)}>{t("dash.closePanel")}</button>
          </div>
          <SftpPanel active={sftpVisible} onerror={err} />
        </div>
      {/if}
      {#if EditorViewC}
        {@const Ed = EditorViewC}
        <div class="sftp-view edit-view" hidden={!editVisible}>
          <div class="sftp-view-head">
            <strong>📝 {t("edit.tab")}</strong>
            <span class="spacer"></span>
            <button type="button" class="secondary" onclick={() => (editVisible = false)}>{t("dash.closePanel")}</button>
          </div>
          <Ed />
        </div>
      {/if}
      {#if tunMounted}
        <div class="sftp-view" hidden={!tunVisible}>
          <div class="sftp-view-head">
            <strong>🔀 {t("tunnel.tab")}</strong>
            <span class="spacer"></span>
            <button type="button" class="secondary" onclick={() => (tunVisible = false)}>{t("dash.closePanel")}</button>
          </div>
          <TunnelsPanel active={tunVisible} onerror={err} />
        </div>
      {/if}
      {#if !sftpVisible && !editVisible && !tunVisible && sessions.list.length === 0}
        <section class="card welcome">
          <h2>HarakaTS</h2>
          <p class="muted">
            {t("welcome.open", { icon: "▶" })}
            {t("welcome.search")}
          </p>
        </section>
      {/if}
    </main>
  </div>

</div>

{#if appState.error}
  <div class="toast-error" role="alert">
    <span class="error">{appState.error}</span>
    <button type="button" class="icon-btn always" title={t("common.close")} onclick={() => (appState.error = null)}>✕</button>
  </div>
{/if}

{#if modal === "editor" && tree}
  <Modal title={editorConnection ? t("editor.title") : t("editor.new")} onclose={() => (modal = null)}>
    {#key editorConnection?.id ?? "new"}
      <ConnectionEditor
        connection={editorConnection}
        folderId={editorFolder}
        {tree}
        onrefresh={refresh}
        onerror={err}
        onclose={() => (modal = null)}
        onopen={(id) => void startSession(id)}
      />
    {/key}
  </Modal>
{:else if modal === "credentials" && tree}
  <Modal title={t("cred.title")} wide onclose={() => (modal = null)}>
    <CredentialManager {tree} onrefresh={refresh} onerror={err} />
  </Modal>
{:else if modal === "about"}
  <Modal title={t("dash.aboutTitle")} wide onclose={() => (modal = null)}>
    <About onerror={err} />
  </Modal>
{:else if modal === "settings"}
  <Modal title={t("settings.title")} onclose={() => (modal = null)}>
    <SettingsPanel onerror={err} />
  </Modal>
{:else if modal === "transfer" && tree}
  <Modal title={t("tabs.transfer")} wide onclose={() => (modal = null)}>
    <ExportImport {tree} onerror={err} onrefresh={loadTree} />
  </Modal>
{:else if modal === "audit"}
  <Modal title={t("tabs.audit")} wide onclose={() => (modal = null)}>
    <AuditView onerror={err} />
  </Modal>
{:else if modal === "macros" && tree}
  <Modal title={t("macro.title")} wide onclose={() => (modal = null)}>
    <MacrosPanel {tree} onrefresh={refresh} onerror={err} />
  </Modal>
{/if}

{#if paletteOpen && tree}
  <Palette
    {tree}
    onclose={() => (paletteOpen = false)}
    onselect={(id) => void startSession(id)}
    onaction={paletteAction}
  />
{/if}

{#if mxOpen}
  <FloatingPanel title={t("multiexec.title")} onclose={() => (mxOpen = false)}>
    <MultiExecPanel onerror={err} />
  </FloatingPanel>
{/if}
