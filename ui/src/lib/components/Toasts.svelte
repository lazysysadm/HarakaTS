<script lang="ts">
  import { toasts } from "../toast.svelte";
</script>

<div class="toasts" role="status" aria-live="polite">
  {#each toasts.list as item (item.id)}
    <div class="toast-ok"><span aria-hidden="true">✓</span> {item.text}</div>
  {/each}
</div>
