<!--
  TopBar.svelte - Minimal top bar with drag region, theme selector, and new project button

  Provides a drag region for window movement, a theme picker, a sidebar toggle,
  and a "+" button for adding new projects.
-->
<script lang="ts">
  import { Palette, Plus, Bot, PanelLeft } from 'lucide-svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import ThemeSelectorModal from './features/settings/ThemeSelectorModal.svelte';
  import AgentDropdown from './features/agents/AgentDropdown.svelte';
  import { navigation } from './navigation.svelte';

  interface Props {
    onToggleSidebar?: () => void;
    sidebarOpen?: boolean;
  }

  let { onToggleSidebar, sidebarOpen = true }: Props = $props();

  let showThemeModal = $state(false);
  let showAgentDropdown = $state(false);

  function handlePlusClick() {
    if (navigation.selectedProjectId) {
      window.dispatchEvent(new CustomEvent('staged:new-branch'));
    } else {
      window.dispatchEvent(new CustomEvent('staged:new-project'));
    }
  }

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

  <button
    class="icon-btn sidebar-toggle"
    class:active={sidebarOpen}
    onclick={() => onToggleSidebar?.()}
    title="Toggle sidebar (⌘B)"
  >
    <PanelLeft size={14} />
  </button>

  <div class="drag-spacer"></div>

  <div class="top-bar-actions">
    <button
      class="icon-btn"
      onclick={handlePlusClick}
      title={navigation.selectedProjectId ? 'New branch (⌘N)' : 'New project (⌘N)'}
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

  .sidebar-toggle.active {
    color: var(--ui-accent);
  }
</style>
