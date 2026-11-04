<!--
  TopBar.svelte - Persistent app bar with drag region and route content

  Provides a drag region for window movement while detail routes register the
  page-specific title, status, navigation, and actions rendered inside it.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import { getWindowSync } from '../../transport';
  import { navigation, popDetailRoute } from './navigation.svelte';
  import { topBar } from './topBarState.svelte';
  import { viewport, watchViewport } from '../../shared/viewport.svelte';
  import { getTrafficLightSpacerWidth, watchWindowChrome } from '../../shared/windowChrome.svelte';
  import { Button } from '$lib/components/ui/button';

  onMount(() => {
    const stopWatchingViewport = watchViewport();
    const stopWatchingWindowChrome = watchWindowChrome();
    return () => {
      stopWatchingViewport();
      stopWatchingWindowChrome();
    };
  });

  function startDrag(e: PointerEvent) {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    const isInteractive = target.closest('button, a, input, [role="button"]');
    if (!isInteractive) {
      e.preventDefault();
      getWindowSync().startDragging();
    }
  }

  let hasTitle = $derived(
    !!topBar.title || !!topBar.subtitle || !!topBar.leading || !!topBar.badges
  );
  let trafficLightSpacerWidth = $derived(getTrafficLightSpacerWidth(viewport.isMobile));
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="top-bar"
  style={`--traffic-light-spacer-width: ${trafficLightSpacerWidth}px`}
  onpointerdown={startDrag}
>
  <div class="traffic-light-spacer"></div>
  <div class="left-actions">
    {#if navigation.canGoBack}
      <Button
        variant="ghost"
        size="sm"
        class="top-bar-action h-[28px] gap-1.5 text-foreground hover:bg-[var(--ui-selection)] hover:text-foreground"
        title={viewport.showShortcutHints ? 'Back (⌘← / Esc)' : 'Back'}
        aria-label="Back"
        onclick={popDetailRoute}
      >
        <ArrowLeft size={14} />
        <span class="top-bar-action-label">Back</span>
      </Button>
    {/if}

    {#if topBar.leftActions}
      {@render topBar.leftActions()}
    {/if}
  </div>

  {#if hasTitle}
    <div class="title-content">
      {#if topBar.leading}
        <div class="leading-slot">
          {@render topBar.leading()}
        </div>
      {/if}
      <div class="title-text">
        {#if topBar.title}
          <div class="title" title={topBar.title}>{topBar.title}</div>
        {/if}
        {#if topBar.subtitle}
          <div class="subtitle" title={topBar.subtitle}>{topBar.subtitle}</div>
        {/if}
      </div>
      {#if topBar.badges}
        <div class="badge-slot">
          {@render topBar.badges()}
        </div>
      {/if}
    </div>
  {/if}

  {#if topBar.center}
    <div class="center-content">
      {@render topBar.center()}
    </div>
  {/if}

  <div class="drag-spacer"></div>

  {#if topBar.rightActions}
    <div class="top-bar-actions">
      {@render topBar.rightActions()}
    </div>
  {/if}
</div>

<style>
  .top-bar {
    position: relative;
    isolation: isolate;
    z-index: var(--z-index-top-bar);
    display: flex;
    align-items: center;
    gap: 8px;
    /* Under the status bar of the home-screen web app (index.html sets
       viewport-fit=cover): the bar's background fills the inset and its
       controls sit below it. The inset is 0 everywhere else. */
    padding: 8px;
    padding-top: calc(8px + env(safe-area-inset-top));
    min-height: calc(42px + env(safe-area-inset-top));
    background: var(--bg-app-bar);
    border-bottom: 1px solid color-mix(in srgb, var(--border-subtle) 70%, transparent);
    flex-shrink: 0;
  }

  .traffic-light-spacer {
    width: var(--traffic-light-spacer-width);
    flex-shrink: 0;
    align-self: stretch;
  }

  .drag-spacer {
    flex: 1;
    align-self: stretch;
    min-width: 20px;
  }

  .left-actions {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .title-content {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    max-width: min(58vw, 720px);
    -webkit-app-region: no-drag;
  }

  .leading-slot,
  .badge-slot {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    flex-shrink: 0;
  }

  .title-text {
    display: flex;
    flex-direction: column;
    justify-content: center;
    min-width: 0;
  }

  .title {
    min-width: 0;
    overflow: hidden;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 650;
    line-height: 1.2;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .center-content {
    position: absolute;
    z-index: 0;
    top: 50%;
    left: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    width: min(42vw, 420px);
    min-width: 0;
    pointer-events: none;
    transform: translate(-50%, -50%);
    -webkit-app-region: drag;
  }

  .subtitle {
    min-width: 0;
    overflow: hidden;
    color: var(--text-muted);
    font-size: calc(var(--size-xs) - 1px);
    line-height: 1.2;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .top-bar-actions {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    -webkit-app-region: no-drag;
  }

  @media (max-width: 768px) {
    .top-bar {
      --top-bar-action-size: 44px;
      padding: 2px 8px;
      padding-top: calc(2px + env(safe-area-inset-top));
    }

    /* Narrow bars can't fit the title and badges (the diff route's RepoLabel)
       beside the right actions, so the badges give way first: they ellipsize
       while the title keeps its width, and once less than 3em (`.badge-slot`'s
       flex-basis) fits beside the title they wrap onto a second line that is
       clipped away, rather than leaving a sliver. Only then does the title
       itself ellipsize. The height cap stops that hidden line from growing the
       bar at larger font sizes, and `clip` (unlike `hidden`) can't be scrolled
       to reveal it. */
    .title-content {
      max-width: min(46vw, 420px);
      max-height: var(--top-bar-action-size);
      align-self: stretch;
      flex-wrap: wrap;
      overflow: clip;
    }

    /* Fill the bar's height, so a wrapped line starts below the clip. */
    .title-text {
      min-height: 100%;
    }

    /* The 3em basis is the wrap threshold; the badge grows back to its natural
       width when there's room. Plain `flex-shrink: 1` would leave a sliver. */
    .badge-slot {
      flex: 1 1 3em;
      max-width: max-content;
    }

    .center-content {
      width: min(34vw, 240px);
    }

    .subtitle {
      display: none;
    }

    /* Every `.top-bar-action` button collapses to an icon-only 44px touch
       target. Sized in px, not rem, so the 13px root font size and the
       font-size preference can't shrink it; unlayered, so it also beats the
       Button size and icon utilities. */
    .top-bar :global(.top-bar-action) {
      width: var(--top-bar-action-size);
      height: var(--top-bar-action-size);
      padding: 0;
    }

    .top-bar :global(.top-bar-action svg) {
      width: 18px;
      height: 18px;
    }

    .top-bar :global(.top-bar-action-label) {
      display: none;
    }
  }
</style>
