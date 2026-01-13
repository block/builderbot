<script lang="ts">
  import { X, Plus, FolderGit2 } from 'lucide-svelte';
  import { windowState, closeTab } from './stores/tabState.svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  interface Props {
    onNewTab: () => void;
    onSwitchTab: (index: number) => Promise<void>;
  }

  let { onNewTab, onSwitchTab }: Props = $props();

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

  async function handleSwitchTab(index: number) {
    console.log(`TabBar: Switching to tab ${index}`);
    await onSwitchTab(index);
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
</script>

<div class="tab-bar">
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
        <FolderGit2 size={14} />
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
</div>

<style>
  .tab-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px 0 8px;
    background: var(--bg-chrome);
    /* border-bottom: 1px solid var(--border-subtle); */
  }

  .tabs {
    position: relative;
    display: flex;
    gap: 2px;
    flex: 1;
    overflow-x: auto;
    scrollbar-width: none; /* Firefox */
  }

  .tabs::-webkit-scrollbar {
    display: none; /* Chrome, Safari */
  }

  .tab-indicator {
    position: absolute;
    top: 0;
    bottom: 0;
    background: var(--bg-primary);
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
    background: var(--bg-chrome);
  }

  .tab-indicator::before {
    left: -12px;
    border-bottom-right-radius: 8px;
    box-shadow: 6px 0 0 0 var(--bg-primary);
  }

  .tab-indicator::after {
    right: -12px;
    border-bottom-left-radius: 8px;
    box-shadow: -6px 0 0 0 var(--bg-primary);
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
    display: flex;
    align-items: center;
    padding: 6px;
    background: var(--bg-primary);
    margin-bottom: 3px;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.1s;
  }

  .new-tab-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }
</style>
