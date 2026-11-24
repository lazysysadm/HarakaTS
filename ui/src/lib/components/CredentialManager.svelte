<script lang="ts">
  import { api, message, type CredentialMeta, type KeyImportDto, type TreeDto } from "../api";
  import { t } from "../i18n.svelte";

  interface Props {
    tree: TreeDto;
    onrefresh: (tree: TreeDto) => void;
    onerror: (msg: string) => void;
  }
  let { tree, onrefresh, onerror }: Props = $props();

  let selectedId = $state<string | null>(null);
  const selected = $derived(tree.credentials.find((c) => c.id === selectedId) ?? null);

  let name = $state("");
  let username = $state("");
  type Mode = "unchanged" | "set" | "clear";
  let pwMode = $state<Mode>("set");
  let pkMode = $state<Mode>("set");
  let ppMode = $state<Mode>("set");
  let pwVal = $state("");
  let pkVal = $state("");
  let ppVal = $state("");
  let revealed = $state<Record<string, string>>({});
  let toast = $state<string | null>(null);
  let pendingDelete = $state<CredentialMeta | null>(null);
  let pendingKey = $state<KeyImportDto | null>(null);
  let busy = $state(false);

  function select(c: CredentialMeta | null) {
    selectedId = c?.id ?? null;
    name = c?.name ?? "";
    username = c?.username ?? "";
    pwMode = c ? "unchanged" : "set";
    pkMode = c ? "unchanged" : "set";
    ppMode = c ? "unchanged" : "set";
    pwVal = "";
    pkVal = "";
    ppVal = "";
    revealed = {};
    pendingKey = null;
  }

  async function importKeyFile() {
    try {
      const dto = await api.credentialImportKeyFile();
      pendingKey = dto;
      pkVal = "";
      if (dto.info.encrypted) ppMode = "set";
      toast = t("cred.pendingSave", { file: dto.file_name });
    } catch (e) {
      onerror(message(e));
    }
  }

  async function run(fn: () => Promise<TreeDto>) {
    busy = true;
    try {
      onrefresh(await fn());
    } catch (e) {
      onerror(message(e));
    } finally {
      busy = false;
    }
  }

  function fieldArg(mode: Mode, value: string, isNew: boolean): string | null | undefined {
    if (isNew) return value.trim() ? value : undefined;
    if (mode === "unchanged") return undefined;
    if (mode === "clear") return null;
    return value;
  }

  async function save() {
    if (!name.trim()) {
      onerror(t("cred.nameRequired"));
      return;
    }
    const isNew = selectedId === null;
    const keyArg = pendingKey ? undefined : fieldArg(pkMode, pkVal, isNew);
    await run(() =>
      api.credentialUpsert(
        selectedId,
        name.trim(),
        username.trim(),
        fieldArg(pwMode, pwVal, isNew),
        keyArg,
        fieldArg(ppMode, ppVal, isNew),
        pendingKey?.pending_id ?? null,
      ),
    );
    pwVal = "";
    pkVal = "";
    ppVal = "";
    revealed = {};
    pendingKey = null;
    if (isNew) select(null);
  }

  async function reveal(field: string) {
    if (!selectedId) return;
    try {
      const value = await api.credentialReveal(selectedId, field);
      revealed = { ...revealed, [field]: value };
      setTimeout(() => {
        const next = { ...revealed };
        delete next[field];
        revealed = next;
      }, 30_000);
    } catch (e) {
      onerror(message(e));
    }
  }

  function hide(field: string) {
    const next = { ...revealed };
    delete next[field];
    revealed = next;
  }

  async function copy(field: string) {
    if (!selectedId) return;
    try {
      const ttl = await api.clipboardCopySecret(selectedId, field);
      toast = ttl > 0 ? `${t("cred.copied")} ${ttl} s` : `${t("cred.copied")} —`;
      setTimeout(() => (toast = null), 4000);
    } catch (e) {
      onerror(message(e));
    }
  }

  async function doDelete(detach: boolean) {
    const target = pendingDelete;
    pendingDelete = null;
    if (!target) return;
    await run(() => api.credentialDelete(target.id, detach));
    if (selectedId === target.id) select(null);
  }
</script>

<section class="card">
  <h2>{t("cred.title")}</h2>
  <div class="cred-layout">
    <div class="cred-list">
      <button type="button" class="secondary" onclick={() => select(null)}>+ {t("cred.new")}</button>
      {#each tree.credentials as c (c.id)}
        <button
          type="button"
          class="tree-row link"
          class:selected={selectedId === c.id}
          onclick={() => select(c)}
        >
          <span>{c.name}</span>
          <span class="muted">({c.username})</span>
          {#if c.needs_reentry.length > 0}<span class="badge warn">{t("cred.needsReentry")}</span>{/if}
          <span class="spacer"></span>
          <span class="muted">{c.ref_count} {t("cred.refs")}</span>
        </button>
      {/each}
    </div>

    <div class="cred-form">
      <label for="cr-name">{t("cred.name")}</label>
      <input id="cr-name" type="text" bind:value={name} />
      <label for="cr-user">{t("cred.username")}</label>
      <input id="cr-user" type="text" bind:value={username} />

      {#each [
        { field: "password", label: t("cred.password"), mode: pwMode, setMode: (m: Mode) => (pwMode = m), val: pwVal, setVal: (v: string) => (pwVal = v), has: selected?.has_password ?? false, area: false },
        { field: "private_key", label: t("cred.privateKey"), mode: pkMode, setMode: (m: Mode) => (pkMode = m), val: pkVal, setVal: (v: string) => (pkVal = v), has: selected?.has_private_key ?? false, area: true },
        { field: "passphrase", label: t("cred.passphrase"), mode: ppMode, setMode: (m: Mode) => (ppMode = m), val: ppVal, setVal: (v: string) => (ppVal = v), has: selected?.has_passphrase ?? false, area: false },
      ] as f (f.field)}
        <div class="secret-field">
          <label for="cr-{f.field}">{f.label}</label>
          {#if selected}
            <select value={f.mode} onchange={(ev) => f.setMode(ev.currentTarget.value as Mode)}>
              <option value="unchanged">{t("cred.unchanged")}</option>
              <option value="set">{t("cred.set")}</option>
              <option value="clear">{t("cred.clear")}</option>
            </select>
          {/if}
          {#if !selected || f.mode === "set"}
            {#if f.area}
              <textarea id="cr-{f.field}" rows="4" value={f.val} oninput={(ev) => f.setVal(ev.currentTarget.value)}></textarea>
              <button type="button" class="secondary" onclick={() => void importKeyFile()}>{t("cred.importFile")}</button>
              {#if pendingKey}
                <p class="ok">
                  {pendingKey.file_name} — {pendingKey.info.algorithm}{#if pendingKey.info.fingerprint} · {pendingKey.info.fingerprint}{/if}{#if pendingKey.info.encrypted} · {t("cred.encrypted")}{/if}
                </p>
                {#if pendingKey.permissions_warning}<p class="error">{pendingKey.permissions_warning}</p>{/if}
                {#if pendingKey.public_key}
                  <p class="muted">{t("cred.publicKeyNear")} <code>{pendingKey.public_key.slice(0, 64)}…</code></p>
                {/if}
              {/if}
            {:else}
              <input id="cr-{f.field}" type="password" value={f.val} oninput={(ev) => f.setVal(ev.currentTarget.value)} autocomplete="new-password" />
            {/if}
          {/if}
          {#if selected && f.has}
            <div class="row">
              {#if revealed[f.field] !== undefined}
                <input type="text" readonly value={revealed[f.field]} />
                <button type="button" class="secondary" onclick={() => hide(f.field)}>{t("cred.hide")}</button>
              {:else}
                <button type="button" class="secondary" onclick={() => void reveal(f.field)}>{t("cred.reveal")}</button>
              {/if}
              <button type="button" class="secondary" onclick={() => void copy(f.field)}>{t("cred.copy")}</button>
            </div>
          {/if}
        </div>
      {/each}

      <button type="button" disabled={busy} onclick={() => void save()}>{t("common.save")}</button>
      {#if selected}
        <button type="button" class="danger" disabled={busy} onclick={() => (pendingDelete = selected)}>{t("cred.delete")}</button>
      {/if}
      {#if toast}<p class="ok">{toast}</p>{/if}
    </div>
  </div>

  {#if pendingDelete}
    <div class="confirm-bar">
      <span>
        {pendingDelete.ref_count > 0
          ? t("cred.detachWarning", { n: pendingDelete.ref_count })
          : t("cred.deleteConfirm", { name: pendingDelete.name })}
      </span>
      <button type="button" class="danger" onclick={() => void doDelete(true)}>{t("common.confirm")}</button>
      <button type="button" class="secondary" onclick={() => (pendingDelete = null)}>{t("common.cancel")}</button>
    </div>
  {/if}
</section>
