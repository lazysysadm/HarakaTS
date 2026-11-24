<script lang="ts">
  import type { TunnelSpec } from "../api";
  import { t } from "../i18n.svelte";
  import { isLoopback, newSpec, specError } from "../tunnels";

  interface Props {
    tunnels: TunnelSpec[];
  }
  let { tunnels = $bindable() }: Props = $props();

  function add() {
    tunnels = [...tunnels, newSpec()];
  }

  function remove(id: string) {
    tunnels = tunnels.filter((x) => x.id !== id);
  }

  function setKind(spec: TunnelSpec, kind: string) {
    spec.kind = kind;
    if (kind === "dynamic") {
      spec.dest_host = null;
      spec.dest_port = null;
      if (spec.bind_port === 8080) spec.bind_port = 1080;
    } else {
      spec.dest_host ??= "localhost";
      spec.dest_port ??= 80;
    }
  }
</script>

<div class="tun-editor">
  <div class="tun-editor-head">
    <strong>{t("tunnel.editor.title")}</strong>
    <button type="button" class="secondary" onclick={add}>+ {t("tunnel.editor.add")}</button>
  </div>
  {#if tunnels.length === 0}
    <p class="muted small">{t("tunnel.editor.empty")}</p>
  {/if}
  {#each tunnels as spec (spec.id)}
    {@const err = specError(spec)}
    <div class="tun-row">
      <select
        aria-label={t("tunnel.kind")}
        value={spec.kind}
        onchange={(ev) => setKind(spec, ev.currentTarget.value)}
      >
        <option value="local">{t("tunnel.kind.local")}</option>
        <option value="remote">{t("tunnel.kind.remote")}</option>
        <option value="dynamic">{t("tunnel.kind.dynamic")}</option>
      </select>
      <input type="text" class="tun-addr" aria-label={t("tunnel.bindAddr")} placeholder="127.0.0.1" bind:value={spec.bind_addr} />
      <input type="number" class="tun-port" min="0" max="65535" aria-label={t("tunnel.bindPort")} bind:value={spec.bind_port} />
      {#if spec.kind !== "dynamic"}
        <span class="tun-arrow">→</span>
        <input type="text" class="tun-addr" aria-label={t("tunnel.destHost")} bind:value={spec.dest_host} />
        <input type="number" class="tun-port" min="1" max="65535" aria-label={t("tunnel.destPort")} bind:value={spec.dest_port} />
      {:else}
        <span class="muted small tun-socks">{t("tunnel.socksHint")}</span>
      {/if}
      <label class="inline tun-auto"><input type="checkbox" bind:checked={spec.auto_start} /> {t("tunnel.autoStart")}</label>
      <button type="button" class="secondary tun-del" title={t("tunnel.editor.remove")} onclick={() => remove(spec.id)}>✕</button>
    </div>
    {#if err}
      <p class="error small tun-msg">{t(err)}</p>
    {/if}
    {#if !isLoopback(spec.bind_addr)}
      <p class="log-warning tun-msg">{t("tunnel.warn.exposed")}</p>
    {/if}
    {#if spec.kind === "remote"}
      <p class="muted small tun-msg">{t("tunnel.remoteHint")}</p>
    {/if}
  {/each}
</div>
