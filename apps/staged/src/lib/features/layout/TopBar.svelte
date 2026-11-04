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
  <div class="traffic-light-spacer" class:collapsed={trafficLightSpacerWidth === 0}></div>
  <div class="left-actions">
    {#if navigation.canGoBack}
      <Button
        variant="ghost"
        size="sm"
        class="top-bar-action h-[28px] gap-1.5 text-foreground hover:bg-[var(--ui-selection)] hover:text-foreground"
        title={viewport.showShortcutHints ? 'Back (⌘← / Esc)' : 'Back'}
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

  /* Labels for buttons that are icon-only on desktop (New project). */
  .top-bar :global(.top-bar-action-label.mobile-only) {
    display: none;
  }

  @media (max-width: 768px) {
    .top-bar {
      --top-bar-action-size: 44px;
      padding: 2px 8px;
      padding-top: calc(2px + env(safe-area-inset-top));
    }

    /* The labelled buttons need the room, so drop the spacers (and their flex
       gaps) and push the right actions over instead. The traffic-light spacer
       goes only when `getTrafficLightSpacerWidth` reserves nothing, so a
       window with traffic lights keeps its room. The bar itself still starts
       window drags through its gaps and the auto margin. */
    .traffic-light-spacer.collapsed,
    .drag-spacer {
      display: none;
    }

    .top-bar-actions {
      margin-left: auto;
    }

    /* Between labelled buttons the title area is narrow, so anything that
       can't fit usefully wraps onto a second line that is clipped away rather
       than leaving a sliver. Badges (status pills, the diff route's RepoLabel)
       are atomic: full width or gone, so a pill is never cut. The title
       ellipsizes down to 4em (`.title-text`'s flex-basis) and only then wraps
       away. The height cap stops the hidden line from growing the bar at
       larger font sizes, and `clip` (unlike `hidden`) can't be scrolled to
       reveal it. */
    .title-content {
      max-width: min(46vw, 420px);
      max-height: var(--top-bar-action-size);
      flex-wrap: wrap;
      overflow: clip;
    }

    /* Zero-width, full-height first item: whatever wraps after it, including
       the title, starts below the clip. The margin cancels its flex gap. */
    .title-content::before {
      content: '';
      height: var(--top-bar-action-size);
      margin-right: -8px;
    }

    /* The 4em basis is the wrap threshold; the title grows back to its natural
       width when there's room. */
    .title-text:has(.title) {
      flex: 1 1 4em;
      max-width: max-content;
    }

    /* A title-less route (the project view) leaves `.title-text` empty, and
       the project view's centre slot stays empty until its heading scrolls
       away. Drop both so their flex gaps don't take room from the badges. */
    .title-text:not(:has(.title)),
    .center-content:not(:has(> *)) {
      display: none;
    }

    /* Centred over the whole bar, the project name would run under the
       labelled buttons; take the space between them instead, and hide it
       once that space is too narrow to read. */
    .center-content {
      position: static;
      flex: 1 1 0;
      width: auto;
      transform: none;
      container-type: inline-size;
    }

    @container (width < 6em) {
      .center-content > :global(*) {
        visibility: hidden;
      }
    }

    .subtitle {
      display: none;
    }

    /* Every `.top-bar-action` button is a 44px-tall touch target that keeps
       its label and grows to fit it; icon-only ones are 44px square. Sized in
       px, not rem, so the 13px root font size and the font-size preference
       can't shrink it; unlayered, so it also beats the Button size utilities.
       The icon keeps the desktop labelled buttons' 1rem. */
    .top-bar :global(.top-bar-action) {
      width: auto;
      min-width: var(--top-bar-action-size);
      height: var(--top-bar-action-size);
      padding-inline: 12px;
    }

    .top-bar :global(.top-bar-action svg) {
      width: 1rem;
      height: 1rem;
    }

    .top-bar :global(.top-bar-action-label.mobile-only) {
      display: inline;
    }

    .top-bar :global(.top-bar-action-label.desktop-only) {
      display: none;
    }
  }
</style>
