<script lang="ts">
  import { onMount } from "svelte";
  import {
    api,
    message,
    type Connection,
    type EffectiveConfig,
    type JumpHop,
    type TreeDto,
  } from "../api";
  import { t, tn } from "../i18n.svelte";
  import TunnelEditor from "./TunnelEditor.svelte";
  import { specError } from "../tunnels";

  interface Props {
    connection: Connection | null;
    folderId: string | null;
    tree: TreeDto;
    onrefresh: (tree: TreeDto) => void;
    onerror: (msg: string) => void;
    onclose: () => void;
    onopen?: (id: string) => void;
  }
  let { connection, folderId, tree, onrefresh, onerror, onclose, onopen }: Props = $props();

  // Snapshot initial volontaire : le composant est re-monté via {#key} à
  // chaque changement de sélection (l'état d'édition ne doit pas suivre la prop).
  // svelte-ignore state_referenced_locally
  const c = connection;
  // svelte-ignore state_referenced_locally
  const initialFolder = folderId;
  let name = $state(c?.name ?? "");
  let host = $state(c?.host ?? "");
  let userOv = $state(c?.user != null);
  let userVal = $state(c?.user ?? "");
  let portOv = $state(c?.port != null);
  let portVal = $state(c?.port ?? 22);
  let credOv = $state(c?.credential_id != null);
  let credVal = $state(c?.credential_id ?? "");
  let clipOv = $state(c?.clipboard_policy != null);
  let clipVal = $state(c?.clipboard_policy ?? "bidirectional");
  let keepOv = $state(c?.keepalive_s != null);
  let keepVal = $state(c?.keepalive_s ?? 30);
  let encOv = $state(c?.encoding != null);
  let encVal = $state(c?.encoding ?? "utf-8");
  let jumpsOv = $state(c?.jump_chain != null);
  let jumpsText = $state(JSON.stringify(c?.jump_chain ?? [], null, 2));
  let tags = $state((c?.tags ?? []).join(", "));
  let color = $state(c?.color ?? "#89b4fa");
  let hasColor = $state(c?.color != null);
  let icon = $state(c?.icon ?? "");
  let favorite = $state(c?.favorite ?? false);
  let startupSnippet = $state(c?.startup_snippet_id ?? "");
  let sessionLogging = $state(c?.session_logging ?? false);
  let notes = $state(c?.notes ?? "");
  let tunnelSpecs = $state(structuredClone($state.snapshot(c?.tunnels ?? [])));
  let folderSel = $state(c?.folder_id ?? initialFolder ?? "");
  let effective = $state<EffectiveConfig | null>(null);
  let busy = $state(false);

  onMount(() => {
    if (connection?.id) {
      api
        .connectionEffective(connection.id)
        .then((e) => (effective = e))
        .catch((e) => onerror(message(e)));
    }
  });

  function provLabel(source: { kind: string; name: string | null }): string {
    if (source.kind === "folder") return `${t("editor.fromFolder")} ${source.name ?? ""}`;
    if (source.kind === "connection") return t("editor.fromConnection");
    return t("editor.fromSystem");
  }

  async function save() {
    if (!name.trim() || !host.trim()) {
      onerror(t("editor.required", { fields: `${t("editor.name")} / ${t("editor.host")}` }));
      return;
    }
    let jumps: JumpHop[] | null = null;
    if (jumpsOv) {
      try {
        jumps = JSON.parse(jumpsText) as JumpHop[];
        if (!Array.isArray(jumps)) throw new Error(t("editor.arrayExpected"));
      } catch {
        onerror(t("editor.jsonInvalid", { field: t("editor.jumps") }));
        return;
      }
    }
    if (tunnelSpecs.some((x) => specError(x))) {
      onerror(t("tunnel.err.invalid"));
      return;
    }
    const targetFolder = folderSel || tree.folders.find((f) => f.parent_id === null)?.id;
    if (!targetFolder) {
      onerror(t("editor.needFolder"));
      return;
    }
    const payload: Connection = {
      id: connection?.id ?? "",
      folder_id: targetFolder,
      name: name.trim(),
      protocol: "ssh",
      host: host.trim(),
      port: portOv ? portVal : null,
      user: userOv && userVal.trim() ? userVal.trim() : null,
      credential_id: credOv && credVal ? credVal : null,
      jump_chain: jumps,
      clipboard_policy: clipOv ? clipVal : null,
      keepalive_s: keepOv ? keepVal : null,
      encoding: encOv && encVal.trim() ? encVal.trim() : null,
      color: hasColor ? color : null,
      icon: icon.trim() ? icon.trim() : null,
      tags: tags.split(",").map((s) => s.trim()).filter(Boolean),
      favorite,
      tunnels: tunnelSpecs,
      startup_snippet_id: startupSnippet || null,
      session_logging: sessionLogging,
      sftp: connection?.sftp ?? { follow_terminal_cwd: false, show_hidden: false },
      notes,
      created_at: connection?.created_at ?? "",
      updated_at: "",
      last_used_at: connection?.last_used_at ?? null,
    };
    busy = true;
    try {
      onrefresh(await api.connectionUpsert(payload));
      onclose();
    } catch (e) {
      onerror(message(e));
    } finally {
      busy = false;
    }
  }
</script>

<section class="card">
  {#if connection && onopen}
    <button type="button" class="connect-big" onclick={() => onopen(connection.id)}>▶ {t("editor.connect")}</button>
  {/if}
  <div class="form-grid">
    <div>
      <label for="ce-name">{t("editor.name")}</label>
      <input id="ce-name" type="text" bind:value={name} />
    </div>
    <div>
      <label for="ce-host">{t("editor.host")}</label>
      <input id="ce-host" type="text" bind:value={host} placeholder="web-01.example.net" />
    </div>
    <div>
      <label for="ce-folder">{t("editor.folder")}</label>
      <select id="ce-folder" bind:value={folderSel}>
        {#each tree.folders as f (f.id)}
          <option value={f.id}>{f.name}</option>
        {/each}
      </select>
    </div>

    <div>
      <label for="ce-user">{t("editor.user")} <label class="inline"><input type="checkbox" bind:checked={userOv} /> {t("editor.inherit")}</label></label>
      <input id="ce-user" type="text" bind:value={userVal} disabled={!userOv} />
    </div>
    <div>
      <label for="ce-port">{t("editor.port")} <label class="inline"><input type="checkbox" bind:checked={portOv} /> {t("editor.inherit")}</label></label>
      <input id="ce-port" type="number" min="1" max="65535" bind:value={portVal} disabled={!portOv} />
    </div>

    <div>
      <label for="ce-cred">{t("editor.credential")} <label class="inline"><input type="checkbox" bind:checked={credOv} /> {t("editor.inherit")}</label></label>
      <select id="ce-cred" bind:value={credVal} disabled={!credOv}>
        <option value="">{t("cred.none")}</option>
        {#each tree.credentials as c (c.id)}
          <option value={c.id}>{c.name} ({c.username})</option>
        {/each}
      </select>
    </div>
    <div>
      <label for="ce-clip">{t("editor.clipboard")} <label class="inline"><input type="checkbox" bind:checked={clipOv} /> {t("editor.inherit")}</label></label>
      <select id="ce-clip" bind:value={clipVal} disabled={!clipOv}>
        <option value="disabled">{t("editor.clipDisabled")}</option>
        <option value="l2r">{t("editor.clipL2r")}</option>
        <option value="r2l">{t("editor.clipR2l")}</option>
        <option value="bidirectional">{t("editor.clipBidi")}</option>
      </select>
    </div>

    <div>
      <label for="ce-keep">{t("editor.keepalive")} <label class="inline"><input type="checkbox" bind:checked={keepOv} /> {t("editor.inherit")}</label></label>
      <input id="ce-keep" type="number" min="0" bind:value={keepVal} disabled={!keepOv} />
    </div>
    <div>
      <label for="ce-enc">{t("editor.encoding")} <label class="inline"><input type="checkbox" bind:checked={encOv} /> {t("editor.inherit")}</label></label>
      <input id="ce-enc" type="text" bind:value={encVal} disabled={!encOv} />
    </div>
  </div>

  <label for="ce-jumps">{t("editor.jumps")} <label class="inline"><input type="checkbox" bind:checked={jumpsOv} /> {t("editor.inherit")}</label></label>
  <textarea id="ce-jumps" rows="4" bind:value={jumpsText} disabled={!jumpsOv}></textarea>

  <div class="form-grid">
    <div>
      <label for="ce-tags">{t("editor.tags")}</label>
      <input id="ce-tags" type="text" bind:value={tags} />
    </div>
    <div>
      <label for="ce-color">{t("editor.color")} <label class="inline"><input type="checkbox" bind:checked={hasColor} /> {t("editor.enable")}</label></label>
      <input id="ce-color" type="color" bind:value={color} disabled={!hasColor} />
    </div>
    <div>
      <label for="ce-icon">{t("editor.icon")}</label>
      <input id="ce-icon" type="text" bind:value={icon} placeholder="server" />
    </div>
    <div>
      <label class="inline"><input type="checkbox" bind:checked={favorite} /> {t("editor.favorite")}</label>
    </div>
  </div>

  <div class="form-grid">
    <div>
      <label for="ce-snippet">{t("macro.startup")}</label>
      <select id="ce-snippet" bind:value={startupSnippet}>
        <option value="">{t("macro.startupNone")}</option>
        {#each tree.macros as m (m.id)}<option value={m.id}>{m.name}</option>{/each}
      </select>
      <p class="muted">{t("macro.startupHint")}</p>
    </div>
    <div>
      <label class="inline"><input type="checkbox" bind:checked={sessionLogging} /> {t("log.session")}</label>
      <p class="log-warning">{t("log.warning")}</p>
    </div>
  </div>

  <TunnelEditor bind:tunnels={tunnelSpecs} />

  <label for="ce-notes">{t("editor.notes")}</label>
  <textarea id="ce-notes" rows="2" bind:value={notes}></textarea>

  {#if effective}
    <h3>{t("editor.effective")}</h3>
    <table>
      <tbody>
        <tr><td>{t("editor.user")}</td><td>{effective.user.value ?? "—"}</td><td class="muted">{provLabel(effective.user.source)}</td></tr>
        <tr><td>{t("editor.port")}</td><td>{effective.port.value ?? "—"}</td><td class="muted">{provLabel(effective.port.source)}</td></tr>
        <tr><td>{t("editor.credential")}</td><td>{tree.credentials.find((c) => c.id === effective?.credential_id.value)?.name ?? "—"}</td><td class="muted">{provLabel(effective.credential_id.source)}</td></tr>
        <tr><td>{t("editor.clipboard")}</td><td>{effective.clipboard_policy.value ?? "—"}</td><td class="muted">{provLabel(effective.clipboard_policy.source)}</td></tr>
        <tr><td>{t("editor.keepalive")}</td><td>{effective.keepalive_s.value ?? "—"}</td><td class="muted">{provLabel(effective.keepalive_s.source)}</td></tr>
        <tr><td>{t("editor.encoding")}</td><td>{effective.encoding.value ?? "—"}</td><td class="muted">{provLabel(effective.encoding.source)}</td></tr>
        <tr><td>{t("editor.jumps")}</td><td>{tn("editor.hops", effective.jump_chain.value?.length ?? 0)}</td><td class="muted">{provLabel(effective.jump_chain.source)}</td></tr>
      </tbody>
    </table>
  {/if}

  <button type="button" disabled={busy} onclick={() => void save()}>{t("editor.save")}</button>
  <button type="button" class="secondary" onclick={onclose}>{t("editor.cancel")}</button>
</section>
