<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    title: string;
    onclose: () => void;
    children?: Snippet;
  }
  let { title, onclose, children }: Props = $props();

  // Panneau flottant NON modal : le reste de l'interface reste utilisable ; déplaçable
  // par l'en-tête, redimensionnable (coin bas-droit), repliable.
  let x = $state(Math.max(16, (typeof window === "undefined" ? 1200 : window.innerWidth) - 456));
  let y = $state(80);
  let collapsed = $state(false);
  let drag: { dx: number; dy: number } | null = null;

  function down(ev: PointerEvent) {
    if ((ev.target as HTMLElement).closest("button")) return;
    drag = { dx: ev.clientX - x, dy: ev.clientY - y };
    (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
  }
  function move(ev: PointerEvent) {
    if (!drag) return;
    x = Math.min(Math.max(0, ev.clientX - drag.dx), window.innerWidth - 80);
    y = Math.min(Math.max(0, ev.clientY - drag.dy), window.innerHeight - 40);
  }
  function up() {
    drag = null;
  }
</script>

<div class="float-panel" class:collapsed style={`left:${x}px;top:${y}px`} role="dialog" aria-label={title}>
  <div class="float-head" role="presentation" onpointerdown={down} onpointermove={move} onpointerup={up}>
    <strong>{title}</strong>
    <span class="spacer"></span>
    <button type="button" class="secondary" onclick={() => (collapsed = !collapsed)}>{collapsed ? "▢" : "–"}</button>
    <button type="button" class="secondary" onclick={onclose}>✕</button>
  </div>
  {#if !collapsed}
    <div class="float-body">{@render children?.()}</div>
  {/if}
</div>
