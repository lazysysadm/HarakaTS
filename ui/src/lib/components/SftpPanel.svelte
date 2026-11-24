<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { t, tn } from "../i18n.svelte";
  import { notify } from "../toast.svelte";
  import { api, message, sftpApi, type TransferSpec } from "../api";
  import { activateSession, connectedSessions, initSessions, sessions as sessStore } from "../sessions.svelte";
  import {
    formatEta,
    formatRate,
    formatSize,
    isDirLike,
    joinPath,
    modeOctal,
    modeString,
    parseOctalMode,
    progressPercent,
    type FileEntry,
  } from "../sftp";
  import { dropFinished, initTransfers, transferList } from "../sftp.svelte";
  import { MAX_INLINE_EDIT } from "../edit";
  import { openRemote } from "../edit.svelte";
  import FilePane from "./FilePane.svelte";

  interface Props {
    /** Faux quand la vue est masquée : les dépôts de fichiers OS sont ignorés. */
    active?: boolean;
    onerror: (msg: string) => void;
  }
  let { active = true, onerror }: Props = $props();

  // Sessions connectées : liste partagée ; la session SFTP suit la session active
  // (terminal focalisé ou choisi dans la barre de sessions).
  const sessions = $derived(connectedSessions());
  let sid = $state<string>("");

  let remotePath = $state("/");
  let remoteHome = $state("/");
  let localHome = $state("/");
  let remoteEntries = $state<FileEntry[]>([]);
  let remoteSel = $state(new Set<string>());
  let remoteLoading = $state(false);
  let remoteError = $state<string | null>(null);

  let localPath = $state("/");
  let localEntries = $state<FileEntry[]>([]);
  let localSel = $state(new Set<string>());
  let localLoading = $state(false);
  let localError = $state<string | null>(null);

  let follow = $state(true);
  let conflict = $state<"skip" | "overwrite">("skip");
  let symlinks = $state<"skip" | "follow">("skip");
  let preserve = $state(true);

  type Action =
    | { kind: "mkdir-remote" | "mkdir-local" | "rename" | "symlink" }
    | { kind: "chmod" }
    | { kind: "delete" }
    | { kind: "props"; entry: FileEntry };
  let action = $state<Action | null>(null);
  let actionText = $state("");
  let actionRecursive = $state(false);
  let info = $state<string | null>(null);

  const list = $derived(transferList());
  const sessionTransfers = $derived(list);
  const doneCount = $derived(list.filter((t) => t.state === "done").length);
  const remoteSelected = $derived(remoteEntries.filter((e) => remoteSel.has(e.path)));
  const localSelected = $derived(localEntries.filter((e) => localSel.has(e.path)));

  const canEdit = $derived(!!sid && remoteSelected.length === 1 && remoteSelected[0].kind === "file");

  function fail(e: unknown) {
    onerror(message(e));
  }

  async function chooseSession(id: string) {
    sid = id;
    remoteSel = new Set();
    remoteEntries = [];
    remoteError = null;
    if (!id) return;
    try {
      remoteHome = await sftpApi.home(id);
      // « Suivre le terminal » : on repart du dernier dossier annoncé par CE shell (conservé
      // même quand son onglet n'était pas affiché), sinon du dossier personnel.
      remotePath = (follow ? sessStore.cwd[id] : undefined) ?? remoteHome;
      await listRemote(remotePath);
    } catch (e) {
      remoteError = message(e);
    }
  }

  async function listRemote(path: string) {
    if (!sid) return;
    remoteLoading = true;
    remoteError = null;
    try {
      const l = await sftpApi.list(sid, path);
      remotePath = l.path;
      remoteEntries = l.entries;
      remoteSel = new Set();
    } catch (e) {
      remoteError = message(e);
    } finally {
      remoteLoading = false;
    }
  }

  async function listLocal(path: string) {
    localLoading = true;
    localError = null;
    try {
      const l = await sftpApi.localList(path);
      localPath = l.path;
      localEntries = l.entries;
      localSel = new Set();
    } catch (e) {
      localError = message(e);
    } finally {
      localLoading = false;
    }
  }

  function activateRemote(e: FileEntry) {
    if (isDirLike(e)) void listRemote(e.path);
    else if (e.kind === "file" && e.size <= MAX_INLINE_EDIT) void editRemote(e, false);
    else void downloadPaths([e.path]);
  }

  async function editRemote(e: FileEntry, external: boolean) {
    if (!sid) return;
    try {
      await openRemote(sid, e.path, external);
    } catch (err) {
      fail(err);
    }
  }

  function activateLocal(e: FileEntry) {
    if (isDirLike(e)) void listLocal(e.path);
    else void uploadPaths([e.path]);
  }

  // ------------------------------------------------------------ transferts

  function spec(direction: "upload" | "download", source: string, dest: string): TransferSpec {
    return { session: sid, direction, source, dest_dir: dest, conflict, symlinks, preserve };
  }

  async function uploadPaths(paths: string[]) {
    if (!sid || paths.length === 0) return;
    try {
      await sftpApi.enqueue(paths.map((p) => spec("upload", p, remotePath)));
      info = tn("sftp.queuedUploads", paths.length);
    } catch (e) {
      fail(e);
    }
  }

  async function downloadPaths(paths: string[]) {
    if (!sid || paths.length === 0) return;
    try {
      await sftpApi.enqueue(paths.map((p) => spec("download", p, localPath)));
      info = tn("sftp.queuedDownloads", paths.length);
    } catch (e) {
      fail(e);
    }
  }

  async function pickAndUpload() {
    try {
      await uploadPaths(await sftpApi.pickFiles());
    } catch (e) {
      fail(e);
    }
  }

  async function pickFolderAndUpload() {
    try {
      const dir = await sftpApi.pickFolder();
      if (dir) await uploadPaths([dir]);
    } catch (e) {
      fail(e);
    }
  }

  async function pickLocalDestination() {
    try {
      const dir = await sftpApi.pickFolder();
      if (dir) await listLocal(dir);
    } catch (e) {
      fail(e);
    }
  }

  function dropOnRemote(from: "local" | "remote", paths: string[]) {
    if (from === "local") void uploadPaths(paths);
  }

  function dropOnLocal(from: "local" | "remote", paths: string[]) {
    if (from === "remote") void downloadPaths(paths);
  }

  // ------------------------------------------------------------ actions CRUD

  function startAction(a: Action) {
    action = a;
    actionRecursive = false;
    const sel = remoteSelected[0];
    if (a.kind === "rename" && sel) actionText = sel.name;
    else if (a.kind === "chmod" && sel) actionText = modeOctal(sel.mode);
    else actionText = "";
  }

  async function showProps() {
    const sel = remoteSelected[0];
    if (!sel || !sid) return;
    try {
      action = { kind: "props", entry: await sftpApi.stat(sid, sel.path) };
    } catch (e) {
      fail(e);
    }
  }

  async function runAction() {
    if (!action || !sid) return;
    const a = action;
    const sel = remoteSelected[0];
    try {
      if (a.kind === "mkdir-remote") {
        if (!actionText.trim()) return;
        await sftpApi.mkdir(sid, joinPath(remotePath, actionText.trim()));
        await listRemote(remotePath);
      } else if (a.kind === "mkdir-local") {
        if (!actionText.trim()) return;
        await sftpApi.localMkdir(joinPath(localPath, actionText.trim()));
        await listLocal(localPath);
      } else if (a.kind === "rename" && sel) {
        const name = actionText.trim();
        if (!name || name.includes("/")) {
          onerror(t("sftp.invalidName"));
          return;
        }
        await sftpApi.rename(sid, sel.path, joinPath(remotePath, name));
        await listRemote(remotePath);
      } else if (a.kind === "chmod" && sel) {
        const mode = parseOctalMode(actionText);
        if (mode === null) {
          onerror(t("sftp.invalidMode"));
          return;
        }
        await sftpApi.chmod(sid, sel.path, mode, actionRecursive);
        await listRemote(remotePath);
      } else if (a.kind === "symlink" && sel) {
        const name = actionText.trim();
        if (!name) return;
        await sftpApi.symlink(sid, joinPath(remotePath, name), sel.path);
        await listRemote(remotePath);
      } else if (a.kind === "delete") {
        for (const e of remoteSelected) {
          await sftpApi.remove(sid, e.path, isDirLike(e) && e.kind === "dir");
        }
        await listRemote(remotePath);
      }
      notify(a.kind === "delete" ? t("toast.deleted") : t("toast.done"));
      action = null;
    } catch (e) {
      fail(e);
      await listRemote(remotePath);
    }
  }

  async function copyPath() {
    const sel = remoteSelected[0];
    if (!sel) return;
    try {
      await api.clipboardCopyText(sel.path);
      info = t("sftp.pathCopied");
      notify(t("toast.copied"));
    } catch (e) {
      fail(e);
    }
  }

  /** Case « Suivre le terminal » : cocher active aussi le hook dans le shell si besoin. */
  function onFollowChange(on: boolean) {
    follow = on;
    if (on && sid && sessStore.cwd[sid] === undefined) void enableFollow();
    else if (on && sid) void listRemote(sessStore.cwd[sid]);
  }

  async function enableFollow() {
    if (!sid) return;
    try {
      await sftpApi.enableFollow(sid);
      info = t("sftp.followEnabled");
      notify(t("toast.done"));
    } catch (e) {
      fail(e);
    }
  }

  async function copySnippet() {
    try {
      await api.clipboardCopyText(await sftpApi.shellSnippet());
      info = t("sftp.snippetCopied");
      notify(t("toast.copied"));
    } catch (e) {
      fail(e);
    }
  }

  async function transferAct(fn: () => Promise<void>) {
    try {
      await fn();
    } catch (e) {
      fail(e);
    }
  }

  async function clearFinished() {
    await transferAct(async () => {
      await sftpApi.clearTransfers();
      dropFinished();
      notify(t("toast.done"));
    });
  }

  // Suit la session active dès qu'elle est connectée ; se vide quand la session disparaît.
  $effect(() => {
    const want = sessStore.active;
    const live = sessions;
    untrack(() => {
      if (want && want !== sid && live.some((s) => s.id === want)) void chooseSession(want);
      else if (sid && !live.some((s) => s.id === sid)) {
        const next = live[0]?.id ?? "";
        void chooseSession(next);
      }
    });
  });

  // Suit le terminal : le dossier annoncé par le shell de la session affichée pilote le panneau.
  $effect(() => {
    const p = sid ? sessStore.cwd[sid] : undefined;
    if (!follow || !p) return;
    untrack(() => {
      if (p !== remotePath) void listRemote(p);
    });
  });

  // Rafraîchit les listings quand un transfert se termine.
  let lastDone = 0;
  $effect(() => {
    const n = doneCount;
    if (n > lastDone) {
      void listRemote(remotePath);
      void listLocal(localPath);
    }
    lastDone = n;
  });

  onMount(() => {
    void initTransfers();
    void initSessions();
    void sftpApi
      .localHome()
      .then((h) => {
        localHome = h;
        return listLocal(h);
      })
      .catch(fail);
    const unsubs: Array<() => void> = [];
    // Glisser des fichiers depuis le gestionnaire de fichiers vers l'app.
    void getCurrentWebview()
      .onDragDropEvent((ev) => {
        if (active && ev.payload.type === "drop" && ev.payload.paths.length > 0) {
          void uploadPaths(ev.payload.paths);
        }
      })
      .then((u) => unsubs.push(u))
      .catch(() => {
        // glisser-déposer OS indisponible : les boutons restent utilisables
      });
    return () => {
      for (const u of unsubs) u();
    };
  });
</script>

<div class="sftp">
  <div class="sftp-top">
    <select
      class="sel-session"
      aria-label={t("sftp.session")}
      value={sid}
      onchange={(ev) => ev.currentTarget.value && void activateSession(ev.currentTarget.value)}
    >
      <option value="">{t("sftp.sessionNone")}</option>
      {#each sessions as s (s.id)}
        <option value={s.id}>{s.title}</option>
      {/each}
    </select>
    <label class="inline chk" title={t("sftp.followTitle")}>
      <input type="checkbox" checked={follow} onchange={(ev) => onFollowChange(ev.currentTarget.checked)} /> {t("sftp.follow")}
    </label>
    <span class="spacer"></span>
    <details class="opts">
      <summary>{t("sftp.options")}</summary>
      <div class="opts-body">
        <label class="inline">{t("sftp.ifExists")}
          <select bind:value={conflict}>
            <option value="skip">{t("sftp.skip")}</option>
            <option value="overwrite">{t("sftp.overwrite")}</option>
          </select>
        </label>
        <label class="inline">{t("sftp.links")}
          <select bind:value={symlinks}>
            <option value="skip">{t("sftp.skip")}</option>
            <option value="follow">{t("sftp.followLinks")}</option>
          </select>
        </label>
        <label class="inline"><input type="checkbox" bind:checked={preserve} /> {t("sftp.preserve")}</label>
        <button type="button" class="tb" title={t("sftp.snippetTitle")} onclick={() => void copySnippet()}>{t("sftp.snippetBtn")}</button>
      </div>
    </details>
  </div>

  {#if !sid}
    <p class="muted small">{t("sftp.noSessionHint", { sftp: t("tabs.sftp") })}</p>
  {/if}

  {#snippet localActions()}
    <button type="button" class="tb primary" disabled={!sid || localSelected.length === 0} title={t("sftp.uploadTitle")} onclick={() => void uploadPaths(localSelected.map((e) => e.path))}>⬆ {t("sftp.upload")}</button>
    <button type="button" class="tb" disabled={!sid} title={t("sftp.pickFilesTitle")} onclick={() => void pickAndUpload()}>{t("sftp.pickFiles")}</button>
    <button type="button" class="tb" disabled={!sid} title={t("sftp.pickFolderTitle")} onclick={() => void pickFolderAndUpload()}>{t("sftp.pickFolder")}</button>
    <button type="button" class="tb" title={t("sftp.newLocalFolder")} onclick={() => startAction({ kind: "mkdir-local" })}>＋ {t("sftp.folder")}</button>
    <button type="button" class="tb" title={t("sftp.browseTitle")} onclick={() => void pickLocalDestination()}>{t("sftp.browse")}</button>
  {/snippet}

  {#snippet remoteActions()}
    <button type="button" class="tb primary" disabled={!sid || remoteSelected.length === 0} title={t("sftp.downloadTitle")} onclick={() => void downloadPaths(remoteSelected.map((e) => e.path))}>⬇ {t("sftp.download")}</button>
    <button type="button" class="tb" disabled={!canEdit} title={t("edit.btnEditTitle")} onclick={() => void editRemote(remoteSelected[0], false)}>📝 {t("edit.btnEdit")}</button>
    <button type="button" class="tb" disabled={!canEdit} title={t("edit.btnExternalTitle")} onclick={() => void editRemote(remoteSelected[0], true)}>↗ {t("edit.btnExternal")}</button>
    <button type="button" class="tb" disabled={!sid} title={t("sftp.newRemoteFolder")} onclick={() => startAction({ kind: "mkdir-remote" })}>＋ {t("sftp.folder")}</button>
    <button type="button" class="tb" disabled={remoteSelected.length !== 1} onclick={() => startAction({ kind: "rename" })}>{t("sftp.rename")}</button>
    <button type="button" class="tb" disabled={remoteSelected.length !== 1} onclick={() => startAction({ kind: "chmod" })}>{t("pane.perms")}</button>
    <button type="button" class="tb" disabled={remoteSelected.length !== 1} onclick={() => startAction({ kind: "symlink" })}>{t("sftp.link")}</button>
    <button type="button" class="tb" disabled={remoteSelected.length !== 1} onclick={() => void showProps()}>{t("sftp.info")}</button>
    <button type="button" class="tb" disabled={remoteSelected.length !== 1} onclick={() => void copyPath()}>{t("sftp.copyPath")}</button>
    <button type="button" class="tb danger" disabled={remoteSelected.length === 0} onclick={() => startAction({ kind: "delete" })}>{t("tree.delete")}</button>
  {/snippet}

  <div class="sftp-panes">
    <FilePane
      title={t("pane.local")}
      side="local"
      home={localHome}
      path={localPath}
      entries={localEntries}
      loading={localLoading}
      error={localError}
      selected={localSel}
      actions={localActions}
      onnavigate={(p) => void listLocal(p)}
      onrefresh={() => void listLocal(localPath)}
      onselect={(s) => (localSel = s)}
      ondropfrom={dropOnLocal}
      onactivate={activateLocal}
    />
    <FilePane
      title={t("pane.remote")}
      side="remote"
      home={remoteHome}
      path={remotePath}
      entries={remoteEntries}
      loading={remoteLoading}
      error={remoteError}
      selected={remoteSel}
      actions={remoteActions}
      onnavigate={(p) => void listRemote(p)}
      onrefresh={() => void listRemote(remotePath)}
      onselect={(s) => (remoteSel = s)}
      ondropfrom={dropOnRemote}
      onactivate={activateRemote}
    />
  </div>

  {#if action}
    <div class="confirm-bar action-bar">
      {#if action.kind === "props"}
        {@const p = action.entry}
        <span>
          <strong>{p.name}</strong> · {p.kind} · {formatSize(p.size)} · {modeString(p.mode)} ({modeOctal(p.mode)})
          · {p.user ?? p.uid ?? "?"}:{p.group ?? p.gid ?? "?"}
          {#if p.link_target} · → {p.link_target}{/if}
        </span>
        <button type="button" class="secondary" onclick={() => (action = null)}>{t("common.close")}</button>
      {:else if action.kind === "delete"}
        <span>
          {tn("sftp.deleteConfirm", remoteSelected.length)}
          {#if remoteSelected.some((e) => e.kind === "dir")}<strong>{t("sftp.deleteRecursive")}</strong>{/if}
        </span>
        <button type="button" class="danger" onclick={() => void runAction()}>{t("common.confirm")}</button>
        <button type="button" class="secondary" onclick={() => (action = null)}>{t("common.cancel")}</button>
      {:else}
        <span>
          {action.kind === "rename" ? t("sftp.newName") : action.kind === "chmod" ? t("sftp.modeOctal") : action.kind === "symlink" ? t("sftp.linkName") : t("sftp.folderName")}
        </span>
        <input
          type="text"
          bind:value={actionText}
          onkeydown={(ev) => {
            if (ev.key === "Enter") void runAction();
            if (ev.key === "Escape") action = null;
          }}
        />
        {#if action.kind === "chmod"}
          <label class="inline"><input type="checkbox" bind:checked={actionRecursive} /> {t("sftp.recursive")}</label>
        {/if}
        <button type="button" onclick={() => void runAction()}>OK</button>
        <button type="button" class="secondary" onclick={() => (action = null)}>{t("common.cancel")}</button>
      {/if}
    </div>
  {/if}

  {#if info}
    <p class="muted small">{info}</p>
  {/if}

  <div class="transfers">
    <div class="pane-head">
      <strong>{t("sftp.transfers", { n: sessionTransfers.length })}</strong>
      <span class="spacer"></span>
      <button type="button" class="tb" onclick={() => void clearFinished()}>{t("sftp.clearFinished")}</button>
    </div>
    {#if sessionTransfers.length === 0}
      <p class="muted">{t("sftp.noTransfers")}</p>
    {/if}
    {#each sessionTransfers as tr (tr.id)}
      <div class="transfer-row" class:err={tr.state === "error"}>
        <span class="t-dir">{tr.direction === "upload" ? "⬆" : "⬇"}</span>
        <span class="t-name" title={tr.source}>{tr.name}</span>
        <div class="bar"><div class="fill" style="width: {progressPercent(tr.done, tr.total)}%"></div></div>
        <span class="muted t-meta">
          {#if tr.state === "running"}
            {progressPercent(tr.done, tr.total)} % · {formatSize(tr.done)}/{formatSize(tr.total)} · {formatRate(tr.bps)} · {formatEta(tr.eta_s)}
            {#if tr.files_total > 1} · {tn("sftp.filesProgress", tr.files_total, { done: tr.files_done })}{/if}
          {:else if tr.state === "done"}
            {t("transfer.done")} · {formatSize(tr.total)}
          {:else if tr.state === "error"}
            {t("transfer.error", { error: tr.error ?? "" })}
          {:else}
            {tr.state === "paused" ? t("transfer.paused") : tr.state === "queued" ? t("transfer.queued") : t("transfer.cancelled")}
            {#if tr.state === "paused"} · {formatSize(tr.done)}/{formatSize(tr.total)}{/if}
          {/if}
        </span>
        <span class="t-actions">
          {#if tr.state === "running" || tr.state === "queued"}
            <button type="button" class="icon-btn always" title={t("transfer.pause")} onclick={() => void transferAct(() => sftpApi.pause(tr.id))}>⏸</button>
          {/if}
          {#if tr.state === "paused" || tr.state === "error"}
            <button type="button" class="icon-btn always" title={t("transfer.resume")} onclick={() => void transferAct(() => sftpApi.resume(tr.id))}>▶</button>
          {/if}
          {#if tr.state === "running" || tr.state === "queued" || tr.state === "paused" || tr.state === "error"}
            <button type="button" class="icon-btn always" title={t("transfer.cancel")} onclick={() => void transferAct(() => sftpApi.cancel(tr.id))}>✕</button>
          {/if}
        </span>
      </div>
    {/each}
  </div>
</div>
