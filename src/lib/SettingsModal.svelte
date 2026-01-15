<!--
  SettingsModal.svelte - User preferences and settings

  Sections:
  1. Layout - Sidebar position toggle
  2. Keyboard - Shortcut customization
-->
<script lang="ts">
  import { X, Settings, PanelLeft, PanelRight, Keyboard, RotateCcw } from 'lucide-svelte';
  import {
    preferences,
    toggleSidebarPosition,
    saveCustomKeyboardBinding,
    removeCustomKeyboardBinding,
    resetAllKeyboardBindings,
    type SidebarPosition,
  } from './stores/preferences.svelte';
  import {
    getAllShortcuts,
    formatShortcutKeys,
    isMac,
    updateBinding,
    resetBinding,
    hasConflict,
    isCustomized,
    type Shortcut,
    type Modifiers,
  } from './services/keyboard';

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  // Active tab
  type Tab = 'layout' | 'keyboard';
  let activeTab = $state<Tab>('layout');

  // Keyboard rebinding state
  let editingShortcutId = $state<string | null>(null);
  let capturedKeys = $state<string[]>([]);
  let capturedModifiers = $state<Modifiers>({});
  let conflictId = $state<string | null>(null);

  // Version counter to force reactivity when shortcuts are reset
  let shortcutVersion = $state(0);

  // Category display names and order
  const categoryInfo: Record<string, { title: string; order: number }> = {
    navigation: { title: 'Navigation', order: 1 },
    view: { title: 'View', order: 2 },
    comments: { title: 'Comments', order: 3 },
    files: { title: 'Files', order: 4 },
  };

  interface ShortcutGroup {
    title: string;
    order: number;
    shortcuts: Shortcut[];
  }

  function getGroupedShortcuts(_version: number): ShortcutGroup[] {
    const allShortcuts = getAllShortcuts();
    const byCategory = new Map<string, Shortcut[]>();

    for (const shortcut of allShortcuts) {
      const copy = { ...shortcut, keys: [...shortcut.keys] };
      const list = byCategory.get(shortcut.category) || [];
      list.push(copy);
      byCategory.set(shortcut.category, list);
    }

    const groups: ShortcutGroup[] = [];

    for (const [category, shortcuts] of byCategory) {
      const info = categoryInfo[category] || { title: category, order: 99 };
      groups.push({
        title: info.title,
        order: info.order,
        shortcuts,
      });
    }

    return groups.sort((a, b) => a.order - b.order);
  }

  let shortcutGroups = $derived(getGroupedShortcuts(shortcutVersion));

  function formatKeys(shortcut: Shortcut): string {
    const formatted = formatShortcutKeys(shortcut);
    return formatted.map((f) => f.modifiers.join('') + f.key).join(' / ');
  }

  function startEditing(id: string) {
    editingShortcutId = id;
    capturedKeys = [];
    capturedModifiers = {};
    conflictId = null;
  }

  function cancelEditing() {
    editingShortcutId = null;
    capturedKeys = [];
    capturedModifiers = {};
    conflictId = null;
  }

  function confirmBinding() {
    if (!editingShortcutId || capturedKeys.length === 0) return;

    // Check for conflicts
    const conflict = hasConflict(capturedKeys, capturedModifiers, editingShortcutId);
    if (conflict) {
      conflictId = conflict;
      return;
    }

    // Update the binding
    updateBinding(editingShortcutId, capturedKeys, capturedModifiers);
    saveCustomKeyboardBinding(editingShortcutId, {
      keys: capturedKeys,
      modifiers: capturedModifiers,
    });

    shortcutVersion++;
    cancelEditing();
  }

  function resetShortcut(id: string) {
    resetBinding(id);
    removeCustomKeyboardBinding(id);
    shortcutVersion++;
  }

  function resetAllShortcuts() {
    const allShortcuts = getAllShortcuts();
    for (const shortcut of allShortcuts) {
      resetBinding(shortcut.id);
    }
    resetAllKeyboardBindings();
    shortcutVersion++;
  }

  function handleKeyCapture(event: KeyboardEvent) {
    if (!editingShortcutId) return;

    event.preventDefault();
    event.stopPropagation();

    // Ignore modifier-only keypresses
    if (['Control', 'Meta', 'Shift', 'Alt'].includes(event.key)) {
      return;
    }

    // Escape cancels
    if (event.key === 'Escape') {
      cancelEditing();
      return;
    }

    // Enter confirms
    if (event.key === 'Enter') {
      confirmBinding();
      return;
    }

    capturedKeys = [event.key];
    capturedModifiers = {
      ctrl: event.ctrlKey,
      meta: event.metaKey,
      shift: event.shiftKey,
      alt: event.altKey,
    };

    // Clear conflict when new key is captured
    conflictId = null;
  }

  function formatCapturedKey(): string {
    if (capturedKeys.length === 0) return '';

    const parts: string[] = [];
    if (isMac()) {
      if (capturedModifiers.ctrl) parts.push('\u2303');
      if (capturedModifiers.alt) parts.push('\u2325');
      if (capturedModifiers.shift) parts.push('\u21e7');
      if (capturedModifiers.meta) parts.push('\u2318');
    } else {
      if (capturedModifiers.ctrl) parts.push('Ctrl+');
      if (capturedModifiers.meta) parts.push('Ctrl+');
      if (capturedModifiers.alt) parts.push('Alt+');
      if (capturedModifiers.shift) parts.push('Shift+');
    }

    let key = capturedKeys[0];
    if (key === 'ArrowDown') key = '\u2193';
    else if (key === 'ArrowUp') key = '\u2191';
    else if (key === 'ArrowLeft') key = '\u2190';
    else if (key === 'ArrowRight') key = '\u2192';
    else key = key.toUpperCase();

    parts.push(key);
    return parts.join('');
  }

  function getConflictDescription(): string {
    if (!conflictId) return '';
    const shortcut = getAllShortcuts().find((s) => s.id === conflictId);
    return shortcut ? shortcut.description : conflictId;
  }

  function handleKeydown(event: KeyboardEvent) {
    // Don't close on Escape if editing
    if (editingShortcutId) return;

    if (event.key === 'Escape') {
      onClose();
      event.preventDefault();
    }
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      if (editingShortcutId) {
        cancelEditing();
      } else {
        onClose();
      }
    }
  }
</script>

<svelte:window onkeydown={editingShortcutId ? handleKeyCapture : handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={(e) => !editingShortcutId && e.key === 'Escape' && onClose()}
>
  <div class="modal">
    <header class="modal-header">
      <h2>
        <Settings size={16} />
        Settings
      </h2>
      <button class="close-btn" onclick={onClose}>
        <X size={16} />
      </button>
    </header>

    <div class="modal-body">
      <!-- Tabs -->
      <div class="tabs">
        <button
          class="tab"
          class:active={activeTab === 'layout'}
          onclick={() => (activeTab = 'layout')}
        >
          <PanelLeft size={14} />
          Layout
        </button>
        <button
          class="tab"
          class:active={activeTab === 'keyboard'}
          onclick={() => (activeTab = 'keyboard')}
        >
          <Keyboard size={14} />
          Keyboard
        </button>
      </div>

      <!-- Layout Tab -->
      {#if activeTab === 'layout'}
        <div class="tab-content">
          <div class="setting-row">
            <div class="setting-info">
              <div class="setting-label">Sidebar Position</div>
              <div class="setting-description">Choose which side the file list appears on</div>
            </div>
            <div class="toggle-group">
              <button
                class="toggle-btn"
                class:active={preferences.sidebarPosition === 'left'}
                onclick={() => preferences.sidebarPosition !== 'left' && toggleSidebarPosition()}
              >
                <PanelLeft size={14} />
                Left
              </button>
              <button
                class="toggle-btn"
                class:active={preferences.sidebarPosition === 'right'}
                onclick={() => preferences.sidebarPosition !== 'right' && toggleSidebarPosition()}
              >
                <PanelRight size={14} />
                Right
              </button>
            </div>
          </div>
        </div>
      {/if}

      <!-- Keyboard Tab -->
      {#if activeTab === 'keyboard'}
        <div class="tab-content keyboard-tab">
          <div class="keyboard-header">
            <span class="keyboard-hint">Click a shortcut to rebind it</span>
            <button class="reset-all-btn" onclick={resetAllShortcuts}>
              <RotateCcw size={12} />
              Reset All
            </button>
          </div>

          <div class="shortcuts-list">
            {#each shortcutGroups as group}
              <div class="shortcut-group">
                <div class="group-title">{group.title}</div>
                {#each group.shortcuts as shortcut}
                  <div class="shortcut-row" class:editing={editingShortcutId === shortcut.id}>
                    <span class="shortcut-description">{shortcut.description}</span>
                    {#if editingShortcutId === shortcut.id}
                      <div class="capture-area">
                        {#if capturedKeys.length > 0}
                          <span class="captured-key" class:conflict={conflictId}>
                            {formatCapturedKey()}
                          </span>
                          {#if conflictId}
                            <span class="conflict-msg"
                              >Conflicts with "{getConflictDescription()}"</span
                            >
                          {/if}
                        {:else}
                          <span class="capture-prompt">Press a key...</span>
                        {/if}
                        <div class="capture-actions">
                          <button class="action-btn" onclick={cancelEditing}>Cancel</button>
                          <button
                            class="action-btn primary"
                            onclick={confirmBinding}
                            disabled={capturedKeys.length === 0 || !!conflictId}
                          >
                            Save
                          </button>
                        </div>
                      </div>
                    {:else}
                      <div class="shortcut-actions">
                        <button class="key-btn" onclick={() => startEditing(shortcut.id)}>
                          {formatKeys(shortcut)}
                        </button>
                        {#if isCustomized(shortcut.id)}
                          <button
                            class="reset-btn"
                            onclick={() => resetShortcut(shortcut.id)}
                            title="Reset to default"
                          >
                            <RotateCcw size={12} />
                          </button>
                        {/if}
                      </div>
                    {/if}
                  </div>
                {/each}
              </div>
            {/each}
          </div>
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 520px;
    max-width: 90vw;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .modal-header h2 {
    margin: 0;
    font-size: var(--size-base);
    font-weight: 600;
    color: var(--text-primary);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .modal-header h2 :global(svg) {
    color: var(--text-muted);
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .close-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .modal-body {
    padding: 0;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
  }

  /* Tabs */
  .tabs {
    display: flex;
    border-bottom: 1px solid var(--border-subtle);
    padding: 0 20px;
  }

  .tab {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 12px 16px;
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
    margin-bottom: -1px;
  }

  .tab:hover {
    color: var(--text-primary);
  }

  .tab.active {
    color: var(--text-primary);
    border-bottom-color: var(--ui-accent);
  }

  .tab :global(svg) {
    opacity: 0.7;
  }

  .tab.active :global(svg) {
    opacity: 1;
  }

  /* Tab Content */
  .tab-content {
    padding: 20px;
  }

  .setting-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
  }

  .setting-info {
    flex: 1;
  }

  .setting-label {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
    margin-bottom: 4px;
  }

  .setting-description {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .toggle-group {
    display: flex;
    gap: 4px;
    background: var(--bg-primary);
    border-radius: 6px;
    padding: 4px;
  }

  .toggle-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .toggle-btn:hover {
    color: var(--text-primary);
  }

  .toggle-btn.active {
    background: var(--bg-chrome);
    color: var(--text-primary);
    box-shadow: var(--shadow-subtle);
  }

  /* Keyboard Tab */
  .keyboard-tab {
    padding: 12px 20px 20px;
  }

  .keyboard-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 12px;
  }

  .keyboard-hint {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .reset-all-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .reset-all-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
  }

  .shortcuts-list {
    max-height: 400px;
    overflow-y: auto;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
  }

  .shortcut-group {
    padding: 8px 12px;
  }

  .shortcut-group:not(:last-child) {
    border-bottom: 1px solid var(--border-subtle);
  }

  .group-title {
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin-bottom: 8px;
  }

  .shortcut-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 0;
    gap: 12px;
  }

  .shortcut-row.editing {
    background: var(--bg-hover);
    margin: 0 -12px;
    padding: 8px 12px;
    border-radius: 4px;
  }

  .shortcut-description {
    font-size: var(--size-xs);
    color: var(--text-primary);
    white-space: nowrap;
  }

  .shortcut-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .key-btn {
    padding: 4px 8px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .key-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
  }

  .reset-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    color: var(--text-faint);
    cursor: pointer;
    transition: color 0.1s;
  }

  .reset-btn:hover {
    color: var(--text-primary);
  }

  .capture-area {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    justify-content: flex-end;
  }

  .capture-prompt {
    font-size: var(--size-xs);
    color: var(--text-faint);
    font-style: italic;
  }

  .captured-key {
    padding: 4px 8px;
    background: var(--ui-accent);
    color: var(--bg-primary);
    border-radius: 4px;
    font-size: var(--size-xs);
    font-weight: 500;
  }

  .captured-key.conflict {
    background: var(--ui-danger);
  }

  .conflict-msg {
    font-size: calc(var(--size-xs) - 1px);
    color: var(--ui-danger);
  }

  .capture-actions {
    display: flex;
    gap: 4px;
  }

  .action-btn {
    padding: 4px 8px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .action-btn:hover:not(:disabled) {
    color: var(--text-primary);
  }

  .action-btn.primary {
    background: var(--ui-accent);
    border-color: var(--ui-accent);
    color: var(--bg-primary);
  }

  .action-btn.primary:hover:not(:disabled) {
    background: var(--ui-accent-hover);
  }

  .action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
