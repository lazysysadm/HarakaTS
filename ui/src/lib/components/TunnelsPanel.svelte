<script lang="ts">
  import {
    api,
    message,
    tunnelApi,
    type AdHocTunnel,
    type Connection,
    type SessionInfo,
    type TunnelKind,
  } from "../api";
  import { t, tn } from "../i18n.svelte";
  import { formatSize } from "../sftp";
  import { bindLabel, destLabel, isLoopback, specError } from "../tunnels";
  import { initTunnels, tunnelList } from "../tunnels.svelte";

  interface Props {
    /** Vue affichée (le composant reste monté une fois ouvert). */
    active: boolean;
    onerror: (msg: string) => void;
  }
  let { active, onerror }: Props = $props();

  let sessions = $state<SessionInfo[]>([]);
  let connections = $state<Connection[]>([]);
  let selected = $state("");
  let kind = $state<TunnelKind>("local");
  let bindAddr = $state("127.0.0.1");
  let bindPort = $state(8080);
  let destHost = $state("localhost");
  let destPort = $state(80);
  let busy = $state(false);

  const live = $derived(tunnelList());
  const connected = $derived(sessions.filter((s) => s.state === "connected"));
  const selectedSession = $derived(connected.find((s) => s.id === selected) ?? null);
  const saved = $derived(
    connections.find((c) => c.id === selectedSession?.connection_id)?.tunnels ?? [],
  );
  const formError = $derived(
    specError({ kind, bind_port: bindPort, dest_host: destHost, dest_port: destPort }),
  );

  function titleOf(id: string): string {
    return sessions.find((s) => s.id === id)?.title ?? id;
  }

  async function reload() {
    try {
      sessions = await api.sessionList();
      connections = (await api.treeGet()).connections;
      if (!connected.some((s) => s.id === selected)) selected = connected[0]?.id ?? "";
    } catch (e) {
      onerror(message(e));
    }
  }

  $effect(() => {
    if (active) {
      void initTunnels();
      void reload();
    }
  });

  function isRunning(id: string): boolean {
    return live.some((x) => x.session === selected && x.id === id && x.state !== "error");
  }

  async function run(action: () => Promise<{ state: string; error: string | null }>) {
    busy = true;
    try {
      const info = await action();
      if (info.state === "error" && info.error) onerror(info.error);
    } catch (e) {
      onerror(message(e));
    } finally {
      busy = false;
    }
  }

  function startSaved(id: string) {
    void run(() => tunnelApi.start(selected, id));
  }

  function startAdHoc() {
    if (!selected || formError) return;
    const spec: AdHocTunnel = {
      kind,
      bind_addr: bindAddr.trim(),
      bind_port: bindPort,
      dest_host: kind === "dynamic" ? null : destHost.trim(),
      dest_port: kind === "dynamic" ? null : destPort,
    };
    void run(() => tunnelApi.startAdHoc(selected, spec));
  }

  async function stop(session: string, id: string) {
    try {
      await tunnelApi.stop(session, id);
    } catch (e) {
      onerror(message(e));
    }
  }
</script>

<div class="tunnels">
  <h4>{t("tunnel.live")}</h4>
  {#if live.length === 0}
    <p class="muted small">{t("tunnel.none")}</p>
  {:else}
    <table class="file-table">
      <thead>
        <tr>
          <th>{t("tunnel.col.session")}</th>
          <th>{t("tunnel.kind")}</th>
          <th>{t("tunnel.col.route")}</th>
          <th>{t("tunnel.col.state")}</th>
          <th class="num">{t("tunnel.col.conns")}</th>
          <th class="num">{t("tunnel.col.bytes")}</th>
          <th></th>
        </tr>
      </thead>
      <tbody>
        {#each live as x (x.session + "\u0000" + x.id)}
          <tr>
            <td class="name">{titleOf(x.session)}</td>
            <td>{t(`tunnel.kind.${x.kind}`)}</td>
            <td class="mono">
              {bindLabel(x.bind_addr, x.bind_port)} → {destLabel(x.kind, x.dest_host, x.dest_port)}
              {#if !isLoopback(x.bind_addr)}<span class="badge warn" title={t("tunnel.warn.exposed")}>{t("tunnel.exposed")}</span>{/if}
            </td>
            <td>
              <span class="badge tun-{x.state}" title={x.error ?? ""}>{t(`tunnel.state.${x.state}`)}</span>
              {#if x.error}<span class="error small">{x.error}</span>{/if}
            </td>
            <td class="num" title={tn("tunnel.connCount", x.active)}>{x.active}</td>
            <td class="num">↑ {formatSize(x.bytes_out)} ↓ {formatSize(x.bytes_in)}</td>
            <td><button type="button" class="tb danger" onclick={() => void stop(x.session, x.id)}>{t("tunnel.stop")}</button></td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  <h4>{t("tunnel.addTitle")}</h4>
  {#if connected.length === 0}
    <p class="muted small">{t("tunnel.noSession")}</p>
  {:else}
    <form
      class="tun-add"
      onsubmit={(ev) => {
        ev.preventDefault();
        startAdHoc();
      }}
    >
      <select aria-label={t("tunnel.col.session")} bind:value={selected}>
        {#each connected as s (s.id)}
          <option value={s.id}>{s.title}</option>
        {/each}
      </select>
      <select aria-label={t("tunnel.kind")} bind:value={kind}>
        <option value="local">{t("tunnel.kind.local")}</option>
        <option value="remote">{t("tunnel.kind.remote")}</option>
        <option value="dynamic">{t("tunnel.kind.dynamic")}</option>
      </select>
      <input type="text" class="tun-addr" aria-label={t("tunnel.bindAddr")} placeholder="127.0.0.1" bind:value={bindAddr} />
      <input type="number" class="tun-port" min="0" max="65535" aria-label={t("tunnel.bindPort")} bind:value={bindPort} />
      {#if kind !== "dynamic"}
        <span class="tun-arrow">→</span>
        <input type="text" class="tun-addr" aria-label={t("tunnel.destHost")} bind:value={destHost} />
        <input type="number" class="tun-port" min="1" max="65535" aria-label={t("tunnel.destPort")} bind:value={destPort} />
      {:else}
        <span class="muted small">{t("tunnel.socksHint")}</span>
      {/if}
      <button type="submit" class="tb" disabled={busy || !!formError}>{t("tunnel.start")}</button>
    </form>
    {#if formError}<p class="error small tun-msg">{t(formError)}</p>{/if}
    {#if !isLoopback(bindAddr)}<p class="log-warning tun-msg">{t("tunnel.warn.exposed")}</p>{/if}
    {#if kind === "remote"}<p class="muted small tun-msg">{t("tunnel.remoteHint")}</p>{/if}

    <h4>{t("tunnel.savedTitle")}</h4>
    {#if saved.length === 0}
      <p class="muted small">{t("tunnel.savedNone")}</p>
    {:else}
      <table class="file-table">
        <tbody>
          {#each saved as s (s.id)}
            <tr>
              <td>{t(`tunnel.kind.${s.kind}`)}</td>
              <td class="mono">{bindLabel(s.bind_addr, s.bind_port)} → {destLabel(s.kind, s.dest_host, s.dest_port)}</td>
              <td class="muted small">{#if s.auto_start}{t("tunnel.autoStart")}{/if}</td>
              <td>
                {#if isRunning(s.id)}
                  <span class="badge">{t("tunnel.running")}</span>
                {:else}
                  <button type="button" class="tb" disabled={busy} onclick={() => startSaved(s.id)}>{t("tunnel.start")}</button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  {/if}
</div>
