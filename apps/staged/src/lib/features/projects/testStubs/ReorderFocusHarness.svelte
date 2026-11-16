<!--
  A keyed list of rows inside a layer host, as the grid, the sidebar and a
  board column render theirs, with keepFocusThroughReorder wired the way they
  wire it. `guard` off leaves the list as it would be without it.
-->
<script lang="ts">
  import { keepFocusThroughReorder } from '../focusThroughReorder.svelte';

  let { ids, guard = true }: { ids: string[]; guard?: boolean } = $props();

  let host = $state<HTMLElement | null>(null);

  function row(id: string): HTMLElement | null {
    for (const el of host?.querySelectorAll<HTMLElement>('[data-row]') ?? []) {
      if (el.dataset.row === id) return el;
    }
    return null;
  }

  // svelte-ignore state_referenced_locally
  if (guard) {
    keepFocusThroughReorder({
      shownIds: () => ids,
      focusedId: () =>
        document.activeElement?.closest<HTMLElement>('[data-row]')?.dataset.row ?? null,
      elementFor: row,
    });
  }
</script>

<div class="workspace-layer" tabindex="-1" bind:this={host}>
  {#each ids as id (id)}
    <button type="button" data-row={id}>{id}</button>
  {/each}
</div>
<button type="button" data-outside>outside</button>
