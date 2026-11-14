<!--
  TopBarNavControls.svelte - Back, Forward and Search, at the top bar's left

  Back and Forward step through the views the window has visited, like a
  browser's buttons (the history lives in navigation.svelte.ts); each is
  disabled when there is no entry that way. Search opens the ⌘K palette
  through the shortcut's own handler.

  Each tooltip names the keys its shortcut is bound to now (shortcutHint), so
  it follows a rebind in Keyboard settings and reads Ctrl off macOS. It is on
  a wrapper: a disabled button takes no pointer events, so one of its own
  would never show. A press on a disabled button lands on that wrapper, and
  TopBar's startDrag ignores the whole group, so it never drags the window.
-->
<script lang="ts">
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import ArrowRight from '@lucide/svelte/icons/arrow-right';
  import Search from '@lucide/svelte/icons/search';
  import { Button } from '$lib/components/ui/button';
  import { viewport } from '../../shared/viewport.svelte';
  import {
    COMMAND_PALETTE_SHORTCUT_ID,
    runCommandPaletteShortcut,
  } from '../commandPalette/paletteState.svelte';
  import { shortcutHint } from '../keyboard/shortcutHints.svelte';
  import { GO_BACK_SHORTCUT_ID, GO_FORWARD_SHORTCUT_ID } from './historyShortcuts';
  import { goBack, goForward, navigation } from './navigation.svelte';

  const BUTTON_CLASS =
    'top-bar-action size-[28px] text-foreground hover:bg-[var(--ui-selection)] hover:text-foreground disabled:opacity-35';

  /** The tooltip: the name, with the shortcut's keys where there is a
   *  keyboard to press them on and the shortcut is registered. */
  function tooltip(name: string, shortcutId: string): string {
    const keys = viewport.showShortcutHints ? shortcutHint(shortcutId) : null;
    return keys ? `${name} (${keys})` : name;
  }
</script>

<div class="top-bar-nav" role="group" aria-label="History and search">
  <span class="inline-flex" title={tooltip('Back', GO_BACK_SHORTCUT_ID)}>
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
  <span
    class="inline-flex"
    class:nowhere-forward={!navigation.canGoForward}
    title={tooltip('Forward', GO_FORWARD_SHORTCUT_ID)}
  >
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
  <span class="inline-flex" title={tooltip('Search projects', COMMAND_PALETTE_SHORTCUT_ID)}>
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

  /* The mobile layout (MOBILE_LAYOUT_QUERY, viewport.svelte.ts), where each
     button is a 44px touch target (TopBar.svelte) and the bar is as narrow
     as 375px. Three of them with gaps left the project view's centre title
     less room than it needs to show at all, so the group gives back what it
     can: no gaps, and no Forward while there is nothing forward to go to,
     which is nearly always. Back and Search keep their places and their
     size; Forward appears between them after a step Back. */
  @media (max-width: 768px),
    (pointer: coarse) and (max-device-width: 500px),
    (pointer: coarse) and (max-device-height: 500px) {
    .top-bar-nav {
      gap: 0;
    }

    .nowhere-forward {
      display: none;
    }

    /* A narrow desktop window takes this layout too, with a mouse and, on a
       Mac, the traffic lights' room ahead of the group (TopBar.svelte). At
       the 375px minimum the three touch-width buttons pushed the route's own
       actions off the bar's right edge. A mouse needs no 44px target, so
       there the buttons keep their desktop width under the bar's 44px
       height. Nested, so the layout's own query stays the one shared form. */
    @media (pointer: fine) {
      .top-bar-nav :global(button.top-bar-action) {
        min-width: 28px;
        padding-inline: 0;
      }
    }
  }
</style>
