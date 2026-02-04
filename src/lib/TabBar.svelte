<script lang="ts">
  import { onMount } from 'svelte';
  import {
    X,
    Plus,
    FolderGit2,
    Loader2,
    Settings2,
    Keyboard,
    Palette,
    ChevronLeft,
  } from 'lucide-svelte';
  import { windowState, closeTab } from './stores/tabState.svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import ThemeSelectorModal from './ThemeSelectorModal.svelte';
  import KeyboardShortcutsModal from './KeyboardShortcutsModal.svelte';
  import SettingsModal from './SettingsModal.svelte';
  import { registerShortcut } from './services/keyboard';

  function startDrag(e: PointerEvent) {
    // Only start drag on left mouse button
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    // Allow drag from elements with drag-region class, or the tab-bar itself
    // but not from interactive elements
    const isInteractive = target.closest('button, a, input, [role="button"]');
    const isDragRegion =
      target.classList.contains('drag-region') || target.classList.contains('tab-bar');
    if (!isInteractive && isDragRegion) {
      e.preventDefault();
      getCurrentWindow().startDragging();
    }
  }

  interface Props {
    onNewTab: () => void;
    onSwitchTab: (index: number) => void;
    onBack?: () => void;
  }

  let { onNewTab, onSwitchTab, onBack }: Props = $props();

  // Modal state
  let showThemeModal = $state(false);
  let showShortcutsModal = $state(false);
  let showSettingsModal = $state(false);

  let tabRefs: (HTMLButtonElement | undefined)[] = $state([]);
  let indicatorStyle = $state('');

  $effect(() => {
    // Update indicator position when active tab changes
    const activeIndex = windowState.activeTabIndex;
    const activeTab = tabRefs[activeIndex];

    if (activeTab) {
      const left = activeTab.offsetLeft;
      const width = activeTab.offsetWidth;
      indicatorStyle = `left: ${left}px; width: ${width}px;`;
    }
  });

  function handleSwitchTab(index: number) {
    console.log(`TabBar: Switching to tab ${index}`);
    onSwitchTab(index);
  }

  async function handleCloseTab(tabId: string, event: MouseEvent | KeyboardEvent) {
    event.stopPropagation();
    closeTab(tabId);

    // Close window if no tabs left
    if (windowState.tabs.length === 0) {
      const window = getCurrentWindow();
      await window.close();
    }
  }

  function handleCloseTabKeydown(tabId: string, event: KeyboardEvent) {
    // Allow Enter or Space to trigger close
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      handleCloseTab(tabId, event);
    }
  }

  function handleNewTab() {
    onNewTab();
  }

  // Register keyboard shortcuts
  onMount(() => {
    const unregisterTheme = registerShortcut({
      id: 'open-theme-picker',
      keys: ['p'],
      modifiers: { meta: true },
      description: 'Theme picker',
      category: 'view',
      handler: () => {
        showThemeModal = !showThemeModal;
      },
    });

    const unregisterSettings = registerShortcut({
      id: 'open-settings',
      keys: [','],
      modifiers: { meta: true },
      description: 'Open settings',
      category: 'view',
      handler: () => {
        showSettingsModal = !showSettingsModal;
      },
    });

    return () => {
      unregisterTheme();
      unregisterSettings();
    };
  });
</script>

<div class="tab-bar drag-region" onpointerdown={startDrag}>
  <div class="traffic-light-spacer drag-region" data-tauri-drag-region></div>
  {#if onBack}
    <button class="back-btn" onclick={onBack}>
      <ChevronLeft size={16} />
      Branches
    </button>
  {:else}
    <div class="tabs">
      <div class="tab-indicator" style={indicatorStyle}></div>
      {#each windowState.tabs as tab, index (tab.id)}
        <button
          bind:this={tabRefs[index]}
          class="tab"
          class:active={index === windowState.activeTabIndex}
          onclick={() => handleSwitchTab(index)}
          title={tab.repoPath}
        >
          {#if tab.agentState?.loading}
            <Loader2 size={14} class="tab-spinner" />
          {:else}
            <FolderGit2 size={14} />
          {/if}
          <span class="tab-name">{tab.repoName}</span>
          {#if windowState.tabs.length > 1}
            <div
              class="close-btn"
              onclick={(e) => handleCloseTab(tab.id, e)}
              onkeydown={(e) => handleCloseTabKeydown(tab.id, e)}
              title="Close tab"
              role="button"
              tabindex="0"
            >
              <X size={12} />
            </div>
          {/if}
        </button>
      {/each}
    </div>

    <button class="new-tab-btn" onclick={handleNewTab} title="Open folder in new tab">
      <Plus size={16} />
    </button>
  {/if}

  <!-- Spacer pushes action buttons to the right -->
  <div class="drag-spacer drag-region" data-tauri-drag-region></div>

  <!-- Action buttons (right side) -->
  <div class="tab-bar-actions">
    <button class="icon-btn" onclick={() => (showSettingsModal = true)} title="Settings (⌘,)">
      <Settings2 size={14} />
    </button>

    <button
      class="icon-btn shortcuts-btn"
      onclick={() => (showShortcutsModal = !showShortcutsModal)}
      title="Keyboard shortcuts"
    >
      <Keyboard size={14} />
    </button>

    <button
      class="icon-btn theme-btn"
      onclick={() => (showThemeModal = !showThemeModal)}
      title="Select theme (⌘P)"
    >
      <Palette size={14} />
    </button>

    <!-- Modals (inside tab-bar-actions for correct absolute positioning) -->
    {#if showThemeModal}
      <ThemeSelectorModal onClose={() => (showThemeModal = false)} />
    {/if}

    {#if showShortcutsModal}
      <KeyboardShortcutsModal onClose={() => (showShortcutsModal = false)} />
    {/if}

    {#if showSettingsModal}
      <SettingsModal onClose={() => (showSettingsModal = false)} />
    {/if}
  </div>
</div>

<style>
  .tab-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 8px 0 8px;
    background: var(--bg-deepest);
    /* border-bottom: 1px solid var(--border-subtle); */
    -webkit-app-region: drag;
  }

  .traffic-light-spacer {
    width: 70px;
    flex-shrink: 0;
    align-self: stretch;
    -webkit-app-region: drag;
  }

  .drag-spacer {
    flex: 1;
    align-self: stretch;
    min-width: 20px;
    -webkit-app-region: drag;
  }

  /* Make interactive elements non-draggable */
  .tab,
  .new-tab-btn,
  .close-btn,
  .icon-btn,
  .back-btn {
    -webkit-app-region: no-drag;
  }

  .back-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 5px 8px;
    background: transparent;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .back-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  /* Tab bar action buttons */
  .tab-bar-actions {
    position: relative;
    display: flex;
    align-items: center;
    gap: 4px;
    margin-bottom: 3px;
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
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .icon-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .tabs {
    position: relative;
    display: flex;
    gap: 2px;
    overflow-x: auto;
    overflow-y: visible;
    scrollbar-width: none; /* Firefox */
    /* Padding to accommodate curved corners on active tab */
    padding: 0 12px;
    margin: 0 -12px;
  }

  .tabs::-webkit-scrollbar {
    display: none; /* Chrome, Safari */
  }

  .tab-indicator {
    position: absolute;
    top: 0;
    bottom: 0;
    background: var(--bg-chrome);
    border-radius: 6px 6px 0 0;
    transition:
      left 0.2s ease,
      width 0.2s ease;
    pointer-events: none;
    z-index: 0;
  }

  /* Curved edges for indicator */
  .tab-indicator::before,
  .tab-indicator::after {
    content: '';
    position: absolute;
    bottom: 0;
    width: 12px;
    height: 12px;
    background: var(--bg-deepest);
  }

  .tab-indicator::before {
    left: -12px;
    border-bottom-right-radius: 8px;
    box-shadow: 6px 0 0 0 var(--bg-chrome);
  }

  .tab-indicator::after {
    right: -12px;
    border-bottom-left-radius: 8px;
    box-shadow: -6px 0 0 0 var(--bg-chrome);
  }

  .tab {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: transparent;
    border: none;
    border-radius: 6px 6px 0 0;
    color: var(--text-muted);
    /* filter: invert(1); */
    font-size: var(--size-sm);
    cursor: pointer;
    transition: color 0.1s;
    white-space: nowrap;
    min-width: 120px;
    max-width: 200px;
  }

  /* Separator between tabs */
  .tab::after {
    content: '';
    position: absolute;
    right: -1px;
    top: 50%;
    transform: translateY(-50%);
    width: 1px;
    height: 16px;
    background: var(--border-subtle);
    transition: opacity 0.1s;
  }

  /* Hide separator for active tab, its neighbors, and last tab */
  .tab.active::after,
  .tab:has(+ .tab.active)::after,
  .tab:last-child::after {
    opacity: 0;
  }

  .tab.active {
    color: var(--text-primary);
    filter: invert(0);
  }

  .tab-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
  }

  .close-btn {
    display: flex;
    align-items: center;
    padding: 2px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-faint);
    cursor: pointer;
    opacity: 0;
    transition: all 0.1s;
  }

  .tab:hover .close-btn {
    opacity: 1;
  }

  .close-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .new-tab-btn {
    position: relative;
    display: flex;
    align-items: center;
    padding: 6px;
    background: transparent;
    margin-bottom: 3px;
    margin-left: 4px;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.1s;
  }

  /* Separator before new tab button */
  .new-tab-btn::before {
    content: '';
    position: absolute;
    left: -5px;
    top: 50%;
    transform: translateY(-50%);
    width: 1px;
    height: 16px;
    background: var(--border-subtle);
  }

  .new-tab-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  /* Spinner animation for agent loading */
  :global(.tab-spinner) {
    animation: spin 1s linear infinite;
    color: var(--text-accent);
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
