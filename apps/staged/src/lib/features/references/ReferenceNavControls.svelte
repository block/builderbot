<script lang="ts">
  import ChevronLeft from '@lucide/svelte/icons/chevron-left';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import { Button } from '$lib/components/ui/button';
  import type { ReferenceNavState } from './referenceHistory.svelte';

  interface Props {
    nav: ReferenceNavState;
  }

  let { nav }: Props = $props();
</script>

<div class="reference-nav-controls" aria-label="Reference history navigation">
  <Button
    variant="ghost"
    size="icon-sm"
    class="size-7 shrink-0 rounded-md text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground disabled:opacity-35 [&_svg]:!size-4"
    title="Back"
    aria-label="Back"
    disabled={!nav.canGoBack}
    onclick={nav.onBack}
  >
    <ChevronLeft size={16} />
  </Button>
  <Button
    variant="ghost"
    size="icon-sm"
    class="size-7 shrink-0 rounded-md text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground disabled:opacity-35 [&_svg]:!size-4"
    title="Forward"
    aria-label="Forward"
    disabled={!nav.canGoForward}
    onclick={nav.onForward}
  >
    <ChevronRight size={16} />
  </Button>
</div>

<style>
  .reference-nav-controls {
    display: flex;
    align-items: center;
    gap: 2px;
    flex-shrink: 0;
  }
</style>
