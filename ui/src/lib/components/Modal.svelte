<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    title: string;
    onclose: () => void;
    wide?: boolean;
    xwide?: boolean;
    children?: Snippet;
  }
  let { title, onclose, wide, xwide, children }: Props = $props();
</script>

<div
  class="modal-overlay"
  role="presentation"
  onclick={(ev) => {
    if (ev.target === ev.currentTarget) onclose();
  }}
  onkeydown={(ev) => {
    if (ev.key === "Escape") onclose();
  }}
>
  <div class="modal" class:modal-wide={wide} class:modal-xwide={xwide} role="dialog" aria-label={title} tabindex="-1">
    <div class="modal-head">
      <h2>{title}</h2>
      <span class="spacer"></span>
      <button type="button" class="secondary" onclick={onclose}>✕</button>
    </div>
    <div class="modal-body">
      {@render children?.()}
    </div>
  </div>
</div>
