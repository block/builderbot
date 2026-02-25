<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { Keyboard, Pencil, RotateCcw, Save, X } from 'lucide-svelte';
  import {
    formatShortcutKeys,
    getAllShortcuts,
    hasShortcutConflict,
    initializeShortcutBindings,
    isMac,
    isShortcutCustomized,
    resetAllShortcutBindings,
    resetShortcutBinding,
    suspendShortcutHandling,
    updateShortcutBinding,
    type Shortcut,
    type ShortcutBinding,
  } from '../keyboard/shortcuts';

  interface ShortcutGroup {
    id: Shortcut['category'];
    title: string;
    order: number;
    shortcuts: Shortcut[];
  }

  const categoryInfo: Record<Shortcut['category'], { title: string; order: number }> = {
    app: { title: 'App', order: 1 },
    search: { title: 'Search', order: 2 },
    view: { title: 'View', order: 3 },
  };

  let shortcutVersion = $state(0);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let editingShortcutId = $state<string | null>(null);
  let capturedBinding = $state<ShortcutBinding | null>(null);
  let conflictId = $state<string | null>(null);
  let busy = $state(false);
  let resumeShortcutHandling: (() => void) | null = null;

  const shortcutGroups = $derived(getShortcutGroups(shortcutVersion));

  onMount(async () => {
    try {
      await initializeShortcutBindings();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
      refreshShortcuts();
    }
  });

  onDestroy(() => {
    releaseShortcutCapture();
  });

  function refreshShortcuts() {
    shortcutVersion += 1;
  }

  function getShortcutGroups(_version: number): ShortcutGroup[] {
    const groups = new Map<Shortcut['category'], Shortcut[]>();
    for (const shortcut of getAllShortcuts()) {
      const list = groups.get(shortcut.category) ?? [];
      list.push(shortcut);
      groups.set(shortcut.category, list);
    }

    return [...groups.entries()]
      .map(([id, shortcuts]) => ({
        id,
        title: categoryInfo[id].title,
        order: categoryInfo[id].order,
        shortcuts,
      }))
      .sort((a, b) => a.order - b.order);
  }

  function releaseShortcutCapture() {
    resumeShortcutHandling?.();
    resumeShortcutHandling = null;
  }

  function startEditing(id: string) {
    if (editingShortcutId === id) return;
    releaseShortcutCapture();
    editingShortcutId = id;
    capturedBinding = null;
    conflictId = null;
    error = null;
    resumeShortcutHandling = suspendShortcutHandling();
  }

  function cancelEditing() {
    editingShortcutId = null;
    capturedBinding = null;
    conflictId = null;
    releaseShortcutCapture();
  }

  function normalizeCapturedKey(key: string): string {
    if (key === 'Spacebar' || key === 'Space') return ' ';
    return key.length === 1 ? key.toLowerCase() : key;
  }

  function captureModifiers(event: KeyboardEvent) {
    const mac = isMac();
    const modifiers = {
      ctrl: mac ? event.ctrlKey : false,
      meta: mac ? event.metaKey : event.ctrlKey || event.metaKey,
      shift: event.shiftKey,
      alt: event.altKey,
    };

    if (!modifiers.ctrl && !modifiers.meta && !modifiers.shift && !modifiers.alt) {
      return undefined;
    }

    return modifiers;
  }

  function handleCaptureKeydown(event: KeyboardEvent) {
    if (!editingShortcutId) return;

    event.preventDefault();
    event.stopPropagation();

    if (event.key === 'Escape') {
      cancelEditing();
      return;
    }

    if (event.key === 'Enter') {
      void confirmBinding();
      return;
    }

    if (
      event.key === 'Meta' ||
      event.key === 'Control' ||
      event.key === 'Shift' ||
      event.key === 'Alt'
    ) {
      return;
    }

    const keys = [normalizeCapturedKey(event.key)];
    const modifiers = captureModifiers(event);
    capturedBinding = { keys, modifiers };
    conflictId = hasShortcutConflict(keys, modifiers, editingShortcutId);
  }

  async function confirmBinding() {
    if (!editingShortcutId || !capturedBinding || busy) return;

    const conflict = hasShortcutConflict(
      capturedBinding.keys,
      capturedBinding.modifiers,
      editingShortcutId
    );
    conflictId = conflict;
    if (conflict) return;

    busy = true;
    error = null;
    try {
      await updateShortcutBinding(
        editingShortcutId,
        capturedBinding.keys,
        capturedBinding.modifiers
      );
      refreshShortcuts();
      cancelEditing();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function handleResetShortcut(id: string) {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await resetShortcutBinding(id);
      refreshShortcuts();
      if (editingShortcutId === id) cancelEditing();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function handleResetAll() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await resetAllShortcutBindings();
      refreshShortcuts();
      cancelEditing();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  function shortcutDescriptionById(id: string): string {
    return getAllShortcuts().find((shortcut) => shortcut.id === id)?.description ?? id;
  }

  function hasAnyCustomBindings(groups: ShortcutGroup[]): boolean {
    return groups.some((group) =>
      group.shortcuts.some((shortcut) => isShortcutCustomized(shortcut.id))
    );
  }
</script>

<svelte:window onkeydown={handleCaptureKeydown} />

<div class="keyboard-settings-panel">
  <div class="panel-intro">
    <div class="intro-copy">
      <h2>
        <Keyboard size={16} />
        Keyboard
      </h2>
      <p>Customize global shortcuts for settings, search, and text size controls.</p>
    </div>

    <button
      class="reset-all-btn"
      disabled={busy || !hasAnyCustomBindings(shortcutGroups)}
      onclick={handleResetAll}
    >
      <RotateCcw size={14} />
      Reset all
    </button>
  </div>

  <div class="panel-body">
    {#if loading}
      <div class="empty-state">Loading keyboard shortcuts…</div>
    {:else if shortcutGroups.length === 0}
      <div class="empty-state">No keyboard shortcuts are currently registered.</div>
    {:else}
      {#if error}
        <div class="error-banner">{error}</div>
      {/if}

      <div class="capture-hint">
        Press a new key combo while editing. Use <strong>Enter</strong> to save and
        <strong>Escape</strong> to cancel.
      </div>

      {#each shortcutGroups as group (group.id)}
        <section class="shortcut-group">
          <h3 class="group-title">{group.title}</h3>

          {#each group.shortcuts as shortcut (shortcut.id)}
            {@const editing = editingShortcutId === shortcut.id}
            {@const customized = isShortcutCustomized(shortcut.id)}
            <div class="shortcut-row" class:editing>
              <div class="shortcut-main">
                <div class="shortcut-label-row">
                  <span class="shortcut-description">{shortcut.description}</span>
                  {#if customized}
                    <span class="custom-badge">Custom</span>
                  {/if}
                </div>

                {#if editing}
                  <div class="capture-row">
                    {#if capturedBinding}
                      <span class="key-list">
                        {#each formatShortcutKeys(capturedBinding) as key, i}
                          {#if i > 0}<span class="separator">/</span>{/if}
                          <span class="key-combo">
                            {#each key.modifiers as mod}
                              <kbd>{mod}</kbd>
                            {/each}
                            <kbd>{key.key}</kbd>
                          </span>
                        {/each}
                      </span>
                    {:else}
                      <span class="capture-placeholder">Press a key combo…</span>
                    {/if}
                  </div>
                {:else}
                  <div class="key-list">
                    {#each formatShortcutKeys(shortcut) as key, i}
                      {#if i > 0}<span class="separator">/</span>{/if}
                      <span class="key-combo">
                        {#each key.modifiers as mod}
                          <kbd>{mod}</kbd>
                        {/each}
                        <kbd>{key.key}</kbd>
                      </span>
                    {/each}
                  </div>
                {/if}
              </div>

              <div class="shortcut-actions">
                {#if editing}
                  <button
                    class="action-btn primary"
                    onclick={confirmBinding}
                    disabled={busy || !capturedBinding || !!conflictId}
                  >
                    <Save size={12} />
                    Save
                  </button>
                  <button class="action-btn" onclick={cancelEditing} disabled={busy}>
                    <X size={12} />
                    Cancel
                  </button>
                {:else}
                  <button
                    class="action-btn"
                    onclick={() => startEditing(shortcut.id)}
                    disabled={busy}
                  >
                    <Pencil size={12} />
                    Edit
                  </button>
                  <button
                    class="action-btn"
                    onclick={() => handleResetShortcut(shortcut.id)}
                    disabled={busy || !customized}
                  >
                    <RotateCcw size={12} />
                    Reset
                  </button>
                {/if}
              </div>
            </div>

            {#if editing && conflictId}
              <p class="conflict-message">
                This binding conflicts with “{shortcutDescriptionById(conflictId)}”.
              </p>
            {/if}
          {/each}
        </section>
      {/each}
    {/if}
  </div>
</div>

<style>
  .keyboard-settings-panel {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border-subtle);
    border-radius: 12px;
    overflow: hidden;
    background: var(--bg-chrome);
  }

  .panel-intro {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 14px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .intro-copy {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }

  .panel-intro h2 {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-md);
    font-weight: 600;
  }

  .panel-intro p {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .panel-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .empty-state {
    min-height: 180px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .capture-hint {
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .shortcut-group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .group-title {
    margin: 0;
    font-size: var(--size-xs);
    font-weight: 600;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .shortcut-row {
    border: 1px solid var(--border-subtle);
    border-radius: 10px;
    padding: 10px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    background: color-mix(in srgb, var(--bg-primary) 88%, transparent);
  }

  .shortcut-row.editing {
    border-color: var(--border-emphasis);
    background: color-mix(in srgb, var(--bg-hover) 55%, transparent);
  }

  .shortcut-main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .shortcut-label-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .shortcut-description {
    font-size: var(--size-sm);
    color: var(--text-primary);
  }

  .custom-badge {
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-muted);
    border: 1px solid var(--border-subtle);
    border-radius: 999px;
    padding: 2px 7px;
    background: color-mix(in srgb, var(--bg-hover) 50%, transparent);
  }

  .shortcut-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .action-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: 1px solid var(--border-muted);
    border-radius: 7px;
    background: transparent;
    color: var(--text-muted);
    font-size: calc(var(--size-xs) - 1px);
    padding: 6px 8px;
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s,
      background-color 0.1s;
  }

  .action-btn:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
    background: color-mix(in srgb, var(--bg-hover) 45%, transparent);
  }

  .action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .action-btn.primary {
    border-color: color-mix(in srgb, var(--ui-accent) 65%, var(--border-muted));
    color: var(--ui-accent);
  }

  .key-list {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .key-combo {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }

  kbd {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 1.8em;
    padding: 0.15em 0.5em;
    border-radius: 6px;
    border: 1px solid var(--border-muted);
    background: var(--bg-primary);
    color: var(--text-muted);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 600;
    line-height: 1.2;
  }

  .separator {
    color: var(--text-faint);
    font-size: calc(var(--size-xs) - 1px);
  }

  .capture-row {
    min-height: 24px;
    display: flex;
    align-items: center;
  }

  .capture-placeholder {
    color: var(--text-faint);
    font-size: var(--size-xs);
  }

  .conflict-message {
    margin: -2px 2px 0;
    font-size: var(--size-xs);
    color: var(--ui-danger);
  }

  .error-banner {
    border: 1px solid color-mix(in srgb, var(--ui-danger) 45%, transparent);
    border-radius: 8px;
    padding: 8px 10px;
    color: var(--ui-danger);
    font-size: var(--size-xs);
    background: color-mix(in srgb, var(--ui-danger) 12%, transparent);
  }

  .reset-all-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 12px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    flex-shrink: 0;
    transition:
      color 0.1s,
      border-color 0.1s,
      background-color 0.1s;
  }

  .reset-all-btn:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
    background: color-mix(in srgb, var(--bg-hover) 45%, transparent);
  }

  .reset-all-btn:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  @media (max-width: 920px) {
    .panel-intro {
      flex-direction: column;
      align-items: flex-start;
    }

    .shortcut-row {
      flex-direction: column;
      align-items: stretch;
    }

    .shortcut-actions {
      justify-content: flex-start;
      flex-wrap: wrap;
    }
  }
</style>
