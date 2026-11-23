<!--
  KeyboardSpacer.svelte — room at the end of a scrolling pane for the
  on-screen keyboard.

  iOS does not shrink the layout viewport for the keyboard, so a pane that
  scrolls keeps its full height and its last rows end up under it: with the
  project list's search field focused, the final results could not be
  scrolled into view. Placed last in the pane's content, this takes up the
  height the keyboard covers (`keyboardInset`), less the home indicator's
  inset the pane already pads for, which the keyboard covers too.

  A block of its own rather than a custom property on the pane: an inherited
  property would restyle every row in the pane each time the inset changed.
  Only touch screens subscribe; nothing else has an on-screen keyboard.
-->
<script lang="ts">
  import { viewport } from './viewport.svelte';
  import { keyboardInset, watchVisualViewport } from './visualViewport.svelte';

  $effect(() => {
    if (viewport.hasCoarsePointer) return watchVisualViewport();
  });
</script>

{#if keyboardInset.px > 0}
  <div
    class="keyboard-spacer"
    aria-hidden="true"
    style:--keyboard-inset="{keyboardInset.px}px"
  ></div>
{/if}

<style>
  .keyboard-spacer {
    flex: none;
    height: max(
      0px,
      calc(
        var(--keyboard-inset) - max(env(safe-area-inset-bottom), var(--safe-bottom-fallback, 0px))
      )
    );
  }
</style>
