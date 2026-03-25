<!--
  TopBar.svelte - Minimal top bar with drag region, theme selector, and new project button

  Provides a drag region for window movement, a theme picker, and a "+" button
  for adding new projects.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { Palette, PanelLeftClose, PanelLeftOpen, Plus, SlidersHorizontal } from 'lucide-svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import ThemeSelectorModal from '../settings/ThemeSelectorModal.svelte';
  import { navigation, openSettings } from './navigation.svelte';
  import {
    hydrateProjectsSidebarState,
    projectsSidebarState,
    setProjectsSidebarCollapsed,
  } from '../projects/projectsSidebarState.svelte';

  let showThemeModal = $state(false);
  const devBranch = import.meta.env.VITE_DEV_BRANCH as string | undefined;

  onMount(() => {
    void hydrateProjectsSidebarState();
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
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="top-bar" onpointerdown={startDrag}>
  <div class="traffic-light-spacer"></div>
  <div class="left-actions">
    <button
      class="icon-btn"
      onclick={toggleProjectsSidebar}
      disabled={!projectsSidebarState.hasProjects}
      title={projectsSidebarState.collapsed ? 'Show projects sidebar' : 'Hide projects sidebar'}
    >
      {#if projectsSidebarState.collapsed || !projectsSidebarState.hasProjects}
        <PanelLeftOpen size={14} />
      {:else}
        <PanelLeftClose size={14} />
      {/if}
    </button>
  </div>
  <div class="drag-spacer"></div>

  <div class="top-bar-actions">
    {#if devBranch}
      <span class="branch-badge">{devBranch}</span>
    {/if}
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

    <button
      class="icon-btn theme-btn"
      onclick={() => (showThemeModal = !showThemeModal)}
      title="Select theme"
    >
      <Palette size={14} />
    </button>

    <button class="icon-btn" onclick={() => openSettings()} title="Settings (⌘,)">
      <SlidersHorizontal size={14} />
    </button>

    {#if showThemeModal}
      <ThemeSelectorModal onClose={() => (showThemeModal = false)} />
    {/if}
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

  .branch-badge {
    font-size: 10px;
    font-weight: 600;
    color: white;
    background: rgb(102, 179, 255, 0.85);
    padding: 2px 8px;
    border-radius: 4px;
    white-space: nowrap;
    max-width: 120px;
    overflow: hidden;
    text-overflow: ellipsis;
    -webkit-app-region: no-drag;
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
</style>
