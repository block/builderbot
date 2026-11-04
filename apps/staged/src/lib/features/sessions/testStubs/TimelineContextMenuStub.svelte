<!--
  Test stub for TimelineContextMenu: renders its children plus a button that
  fires `onNewSessionReferring` with the first action's hashtag ref, so
  component tests can drive the "new session referring" path without the
  real context-menu gestures.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    actions = [],
    onNewSessionReferring,
    children,
  }: {
    actions?: { hashtagRef?: string }[];
    onNewSessionReferring?: (hashtagRef: string) => void;
    children?: Snippet;
  } = $props();

  let ref = $derived(actions.find((action) => action.hashtagRef)?.hashtagRef);
</script>

{#if ref && onNewSessionReferring}
  <button
    type="button"
    data-testid="new-session-referring"
    onclick={() => onNewSessionReferring(ref)}
  >
    New session referring
  </button>
{/if}
{@render children?.()}
