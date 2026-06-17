<script lang="ts">
  import type { Snippet } from 'svelte';
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

  $effect(() => {
    return registerTopBar({
      title,
      subtitle,
      leading: leading ?? null,
      center: center ?? null,
      badges: badges ?? null,
      leftActions: leftActions ?? null,
      rightActions: rightActions ?? null,
    });
  });
</script>
