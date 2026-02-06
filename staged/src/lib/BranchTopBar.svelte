<!--
  BranchTopBar.svelte - Minimal top bar for branch home view
  
  Provides a drag region for window movement and settings access.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { Settings2, Keyboard, Palette, FolderPlus, FileDiff } from 'lucide-svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import ThemeSelectorModal from './ThemeSelectorModal.svelte';
  import KeyboardShortcutsModal from './KeyboardShortcutsModal.svelte';
  import SettingsModal from './SettingsModal.svelte';
  import { registerShortcut } from './services/keyboard';

  interface Props {
    onAddProject?: () => void;
    onToggleMode?: () => void;
  }

  let { onAddProject, onToggleMode }: Props = $props();

  function startDrag(e: PointerEvent) {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    const isInteractive = target.closest('button, a, input, [role="button"]');
    const isDragRegion =
      target.classList.contains('drag-region') || target.classList.contains('top-bar');
    if (!isInteractive && isDragRegion) {
      e.preventDefault();
      getCurrentWindow().startDragging();
    }
  }

  // Modal state
  let showThemeModal = $state(false);
  let showShortcutsModal = $state(false);
  let showSettingsModal = $state(false);

  // Register keyboard shortcuts
  onMount(() => {
    const unregisterTheme = registerShortcut({
      id: 'branch-open-theme-picker',
      keys: ['p'],
      modifiers: { meta: true },
      description: 'Theme picker',
      category: 'view',
      handler: () => {
        showThemeModal = !showThemeModal;
      },
    });

    const unregisterSettings = registerShortcut({
      id: 'branch-open-settings',
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

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="top-bar drag-region" onpointerdown={startDrag}>
  <div class="traffic-light-spacer drag-region" data-tauri-drag-region></div>

  <!-- Spacer pushes action buttons to the right -->
  <div class="drag-spacer drag-region" data-tauri-drag-region></div>

  <!-- Action buttons (right side) -->
  <div class="top-bar-actions">
    {#if onAddProject}
      <button class="add-project-btn" onclick={onAddProject} title="Add Project">
        <FolderPlus size={14} />
        Add Project
      </button>
    {/if}

    {#if onToggleMode}
      <button class="icon-btn" onclick={onToggleMode} title="Switch to Diff view">
        <FileDiff size={14} />
      </button>
    {/if}

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

    <!-- Modals -->
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
  .top-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px;
    background: var(--bg-chrome);
    -webkit-app-region: drag;
    flex-shrink: 0;
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
  .icon-btn,
  .add-project-btn {
    -webkit-app-region: no-drag;
  }

  .add-project-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 10px;
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    border-radius: 6px;
    color: var(--text-secondary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .add-project-btn:hover {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
    background-color: var(--bg-hover);
  }

  /* Action buttons */
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
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .icon-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }
</style>
