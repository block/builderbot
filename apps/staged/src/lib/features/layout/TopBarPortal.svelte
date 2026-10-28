<script lang="ts">
  import { onDestroy, type Snippet } from 'svelte';
  import { registerTopBar } from './topBarState.svelte';
  import { useWorkspaceLayer } from './workspaceLayer';

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
  // The registration follows the layer's `active`, not `visible`: settings and
  // the diff view register later and win while the workspace is parked anyway,
  // and a parked layer flipping inactive would otherwise blank the bar.
  const layer = useWorkspaceLayer();
  const content = () => ({
    title,
    subtitle,
    leading: leading ?? null,
    center: center ?? null,
    badges: badges ?? null,
    leftActions: leftActions ?? null,
    rightActions: rightActions ?? null,
  });
  const handle = registerTopBar(content(), layer.active);
  $effect(() => {
    handle.update(content(), layer.active);
  });
  onDestroy(() => handle.release());
</script>
