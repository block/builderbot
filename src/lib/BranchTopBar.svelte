<!--
  BranchTopBar.svelte - Minimal top bar for branch home view
  
  Provides a drag region for window movement and settings access.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { Settings2, Keyboard, Palette } from 'lucide-svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import ThemeSelectorModal from './ThemeSelectorModal.svelte';
  import KeyboardShortcutsModal from './KeyboardShortcutsModal.svelte';
  import SettingsModal from './SettingsModal.svelte';
  import { registerShortcut } from './services/keyboard';

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

<div class="top-bar drag-region" onpointerdown={startDrag}>
  <div class="traffic-light-spacer drag-region" data-tauri-drag-region></div>

  <!-- Spacer pushes action buttons to the right -->
  <div class="drag-spacer drag-region" data-tauri-drag-region></div>

  <!-- Action buttons (right side) -->
  <div class="top-bar-actions">
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
  .icon-btn {
    -webkit-app-region: no-drag;
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
