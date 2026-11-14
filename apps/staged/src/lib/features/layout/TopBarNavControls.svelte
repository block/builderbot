<!--
  TopBarNavControls.svelte - Back, Forward and Search, at the top bar's left

  Back and Forward step through the views the window has visited, like a
  browser's buttons (the history lives in navigation.svelte.ts); each is
  disabled when there is no entry that way. Search opens the ⌘K palette
  through the shortcut's own handler.

  Each tooltip is on a wrapper: a disabled button takes no pointer events,
  so one of its own would never show. For the same reason a press on a
  disabled button lands on the bar and drags the window, as on any other
  empty part of it.
-->
<script lang="ts">
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import ArrowRight from '@lucide/svelte/icons/arrow-right';
  import Search from '@lucide/svelte/icons/search';
  import { Button } from '$lib/components/ui/button';
  import { viewport } from '../../shared/viewport.svelte';
  import { runCommandPaletteShortcut } from '../commandPalette/paletteState.svelte';
  import { goBack, goForward, navigation } from './navigation.svelte';

  const BUTTON_CLASS =
    'top-bar-action size-[28px] text-foreground hover:bg-[var(--ui-selection)] hover:text-foreground disabled:opacity-35';
</script>

<div class="top-bar-nav" role="group" aria-label="History and search">
  <span class="inline-flex" title={viewport.showShortcutHints ? 'Back (⌘[)' : 'Back'}>
    <Button
      variant="ghost"
      size="icon-sm"
      class={BUTTON_CLASS}
      aria-label="Back"
      disabled={!navigation.canGoBack}
      onclick={() => void goBack()}
    >
      <ArrowLeft size={14} />
    </Button>
  </span>
  <span class="inline-flex" title={viewport.showShortcutHints ? 'Forward (⌘])' : 'Forward'}>
    <Button
      variant="ghost"
      size="icon-sm"
      class={BUTTON_CLASS}
      aria-label="Forward"
      disabled={!navigation.canGoForward}
      onclick={() => void goForward()}
    >
      <ArrowRight size={14} />
    </Button>
  </span>
  <span
    class="inline-flex"
    title={viewport.showShortcutHints ? 'Search projects (⌘K)' : 'Search projects'}
  >
    <Button
      variant="ghost"
      size="icon-sm"
      class={BUTTON_CLASS}
      aria-label="Search projects"
      aria-haspopup="dialog"
      onclick={runCommandPaletteShortcut}
    >
      <Search size={14} />
    </Button>
  </span>
</div>

<style>
  .top-bar-nav {
    display: flex;
    align-items: center;
    gap: 2px;
    flex-shrink: 0;
  }
</style>
