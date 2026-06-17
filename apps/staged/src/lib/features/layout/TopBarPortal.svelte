<script lang="ts">
  import type { Snippet } from 'svelte';
  import { registerTopBar } from './topBarState.svelte';

  interface Props {
    title?: string;
    subtitle?: string | null;
    leading?: Snippet;
    badges?: Snippet;
    leftActions?: Snippet;
    rightActions?: Snippet;
  }

  let { title = '', subtitle = null, leading, badges, leftActions, rightActions }: Props = $props();

  $effect(() => {
    return registerTopBar({
      title,
      subtitle,
      leading: leading ?? null,
      badges: badges ?? null,
      leftActions: leftActions ?? null,
      rightActions: rightActions ?? null,
    });
  });
</script>
