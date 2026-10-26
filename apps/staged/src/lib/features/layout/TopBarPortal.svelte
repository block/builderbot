<script lang="ts">
  import { onDestroy, type Snippet } from 'svelte';
  import { registerTopBar } from './topBarState.svelte';

  interface Props {
    title?: string;
    subtitle?: string | null;
    leading?: Snippet;
    center?: Snippet;
    badges?: Snippet;
    leftActions?: Snippet;
    rightActions?: Snippet;
  }

  let {
    title = '',
    subtitle = null,
    leading,
    center,
    badges,
    leftActions,
    rightActions,
  }: Props = $props();

  // One registration per instance, taken at init so stacking follows mount
  // order; prop changes update it in place rather than re-registering, which
  // would let a parked view's title change hijack the bar from the view on top.
  const content = () => ({
    title,
    subtitle,
    leading: leading ?? null,
    center: center ?? null,
    badges: badges ?? null,
    leftActions: leftActions ?? null,
    rightActions: rightActions ?? null,
  });
  const handle = registerTopBar(content());
  $effect(() => {
    handle.update(content());
  });
  onDestroy(() => handle.release());
</script>
