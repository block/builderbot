<!--
  TopBar.svelte - Minimal top bar with drag region, settings, and new project button

  Provides a drag region for window movement, a "+" button for adding new
  projects, and a settings button.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { PanelLeftClose, PanelLeftOpen, Plus, SlidersHorizontal } from 'lucide-svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { navigation, openSettings } from './navigation.svelte';
  import {
    hydrateProjectsSidebarState,
    projectsSidebarState,
    setProjectsSidebarCollapsed,
  } from '../projects/projectsSidebarState.svelte';
  import { viewport, watchViewport } from '../../shared/viewport.svelte';

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
      getCurrentWindow().startDragging();
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
      <button
        class="icon-btn"
        onclick={toggleProjectsSidebar}
        disabled={!projectsSidebarState.hasProjects}
        title={sidebarOpen ? 'Hide projects sidebar' : 'Show projects sidebar'}
      >
        {#if !sidebarOpen || !projectsSidebarState.hasProjects}
          <PanelLeftOpen size={14} />
        {:else}
          <PanelLeftClose size={14} />
        {/if}
      </button>
    </div>
  {/if}
  <div class="drag-spacer"></div>

  <div class="top-bar-actions">
    <button
      class="icon-btn"
      onclick={() => window.dispatchEvent(new CustomEvent('staged:new-project'))}
      disabled={navigation.activeView === 'settings'}
      title={navigation.activeView === 'settings'
        ? 'Unavailable while viewing settings'
        : 'New project (⌘N)'}
    >
      <Plus size={14} />
    </button>

    <button class="icon-btn" onclick={() => openSettings()} title="Settings (⌘,)">
      <SlidersHorizontal size={14} />
    </button>
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

  .icon-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 5px;
    background: transparent;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    -webkit-app-region: no-drag;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .icon-btn:hover:not(:disabled) {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .icon-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }

  @media (max-width: 768px) {
    .top-bar {
      padding: 6px 8px;
    }

    .traffic-light-spacer {
      width: 58px;
    }

    .icon-btn {
      width: 40px;
      height: 40px;
      padding: 0;
    }
  }
</style>
