<!--
  TopBar.svelte - Minimal top bar with drag region, settings, and new project button

  Provides a drag region for window movement, a "+" button for adding new
  projects, and a settings button.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import PanelLeftClose from '@lucide/svelte/icons/panel-left-close';
  import PanelLeftOpen from '@lucide/svelte/icons/panel-left-open';
  import Plus from '@lucide/svelte/icons/plus';
  import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
  import { getWindowSync } from '../../transport';
  import { navigation, openSettings } from './navigation.svelte';
  import {
    hydrateProjectsSidebarState,
    projectsSidebarState,
    setProjectsSidebarCollapsed,
  } from '../projects/projectsSidebarState.svelte';
  import { viewport, watchViewport } from '../../shared/viewport.svelte';
  import { Button } from '$lib/components/ui/button';
  import * as Tooltip from '$lib/components/ui/tooltip';

  onMount(() => {
    const stopWatchingViewport = watchViewport();
    void hydrateProjectsSidebarState();
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

  function toggleProjectsSidebar() {
    setProjectsSidebarCollapsed(!projectsSidebarState.collapsed);
  }

  let sidebarOpen = $derived(!projectsSidebarState.collapsed);
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="top-bar" onpointerdown={startDrag}>
  <div class="traffic-light-spacer"></div>
  {#if !viewport.isMobile}
    <div class="left-actions">
      <Tooltip.Root>
        <Tooltip.Trigger>
          {#snippet child({ props })}
            <span {...props} class="inline-flex">
              <Button
                variant="ghost"
                size="icon-xs"
                class="max-md:size-10 [&_svg]:size-3.5"
                onclick={toggleProjectsSidebar}
                disabled={!projectsSidebarState.hasProjects}
              >
                {#if !sidebarOpen || !projectsSidebarState.hasProjects}
                  <PanelLeftOpen size={14} />
                {:else}
                  <PanelLeftClose size={14} />
                {/if}
              </Button>
            </span>
          {/snippet}
        </Tooltip.Trigger>
        <Tooltip.Content>
          {sidebarOpen ? 'Hide projects sidebar' : 'Show projects sidebar'}
        </Tooltip.Content>
      </Tooltip.Root>
    </div>
  {/if}
  <div class="drag-spacer"></div>

  <div class="top-bar-actions">
    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <span {...props} class="inline-flex">
            <Button
              variant="ghost"
              size="icon-xs"
              class="max-md:size-10 [&_svg]:size-3.5"
              onclick={() => window.dispatchEvent(new CustomEvent('staged:new-project'))}
              disabled={navigation.activeView === 'settings'}
            >
              <Plus size={14} />
            </Button>
          </span>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>
        {navigation.activeView === 'settings'
          ? 'Unavailable while viewing settings'
          : viewport.showShortcutHints
            ? 'New project (⌘N)'
            : 'New project'}
      </Tooltip.Content>
    </Tooltip.Root>

    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon-xs"
            class="max-md:size-10 [&_svg]:size-3.5"
            onclick={() => openSettings()}
          >
            <SlidersHorizontal size={14} />
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>
        {viewport.showShortcutHints ? 'Settings (⌘,)' : 'Settings'}
      </Tooltip.Content>
    </Tooltip.Root>
  </div>
</div>

<style>
  .top-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px;
    background: var(--bg-chrome);
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
  }

  .top-bar-actions {
    position: relative;
    display: flex;
    align-items: center;
    gap: 4px;
  }

  @media (max-width: 768px) {
    .top-bar {
      padding: 6px 8px;
    }

    .traffic-light-spacer {
      width: 58px;
    }
  }
</style>
