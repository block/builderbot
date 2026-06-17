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
  import { Button } from '$lib/components/ui/button';

  onMount(() => {
    const stopWatchingViewport = watchViewport();
    return () => {
      stopWatchingViewport();
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
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="top-bar" onpointerdown={startDrag}>
  <div class="traffic-light-spacer"></div>
  <div class="left-actions">
    {#if navigation.canGoBack}
      <Button
        variant="ghost"
        size="icon-xs"
        class="max-md:size-10 [&_svg]:size-3.5"
        title={viewport.showShortcutHints ? 'Back (⌘← / Esc)' : 'Back'}
        aria-label="Back"
        onclick={popDetailRoute}
      >
        <ArrowLeft size={14} />
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

  <div class="drag-spacer"></div>

  {#if topBar.rightActions}
    <div class="top-bar-actions">
      {@render topBar.rightActions()}
    </div>
  {/if}
</div>

<style>
  .top-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px;
    min-height: 42px;
    background: var(--bg-app-bar);
    border-bottom: 1px solid color-mix(in srgb, var(--border-subtle) 70%, transparent);
    flex-shrink: 0;
  }

  .traffic-light-spacer {
    width: 70px;
    flex-shrink: 0;
    align-self: stretch;
  }

  .drag-spacer {
    flex: 1;
    align-self: stretch;
    min-width: 20px;
  }

  .left-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .title-content {
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
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    -webkit-app-region: no-drag;
  }

  @media (max-width: 768px) {
    .top-bar {
      padding: 6px 8px;
    }

    .traffic-light-spacer {
      width: 58px;
    }

    .title-content {
      max-width: min(46vw, 420px);
    }

    .subtitle {
      display: none;
    }
  }
</style>
