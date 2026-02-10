<!--
  TopBar.svelte - Minimal top bar with drag region, theme selector, and new project button

  Provides a drag region for window movement, a theme picker, and a "+" button
  for adding new projects.
-->
<script lang="ts">
  import { Palette, Plus, Bot } from 'lucide-svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import ThemeSelectorModal from './ThemeSelectorModal.svelte';
  import AgentDropdown from './AgentDropdown.svelte';

  let showThemeModal = $state(false);
  let showAgentDropdown = $state(false);

  function startDrag(e: PointerEvent) {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    const isInteractive = target.closest('button, a, input, [role="button"]');
    if (!isInteractive) {
      e.preventDefault();
      getCurrentWindow().startDragging();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="top-bar" onpointerdown={startDrag}>
  <div class="traffic-light-spacer"></div>
  <div class="drag-spacer"></div>

  <div class="top-bar-actions">
    <button
      class="icon-btn"
      onclick={() => window.dispatchEvent(new CustomEvent('staged:new-project'))}
      title="New project (⌘N)"
    >
      <Plus size={14} />
    </button>

    <button
      class="icon-btn agent-btn"
      onclick={() => (showAgentDropdown = !showAgentDropdown)}
      title="AI agents"
    >
      <Bot size={14} />
    </button>

    <button
      class="icon-btn theme-btn"
      onclick={() => (showThemeModal = !showThemeModal)}
      title="Select theme"
    >
      <Palette size={14} />
    </button>

    {#if showAgentDropdown}
      <AgentDropdown onClose={() => (showAgentDropdown = false)} />
    {/if}

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

  .icon-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }
</style>
