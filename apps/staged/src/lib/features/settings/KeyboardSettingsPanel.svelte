<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import Keyboard from '@lucide/svelte/icons/keyboard';
  import Pencil from '@lucide/svelte/icons/pencil';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  import Save from '@lucide/svelte/icons/save';
  import X from '@lucide/svelte/icons/x';
  import { Button } from '$lib/components/ui/button';
  import { Badge } from '$lib/components/ui/badge';
  import * as Alert from '$lib/components/ui/alert';
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
  let captureError = $state<string | null>(null);
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
    captureError = null;
    error = null;
    resumeShortcutHandling = suspendShortcutHandling();
  }

  function cancelEditing() {
    editingShortcutId = null;
    capturedBinding = null;
    conflictId = null;
    captureError = null;
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

  function hasPrimaryModifier(modifiers: ShortcutBinding['modifiers']): boolean {
    return !!modifiers?.ctrl || !!modifiers?.meta || !!modifiers?.alt;
  }

  function isPrintableKey(key: string): boolean {
    return key.length === 1;
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

    const capturedKey = normalizeCapturedKey(event.key);
    const keys = [capturedKey];
    const modifiers = captureModifiers(event);

    if (isPrintableKey(capturedKey) && !hasPrimaryModifier(modifiers)) {
      capturedBinding = null;
      conflictId = null;
      captureError = 'Printable shortcuts must include Ctrl/Cmd or Alt.';
      return;
    }

    captureError = null;
    capturedBinding = { keys, modifiers };
    conflictId = hasShortcutConflict(keys, modifiers, editingShortcutId);
  }

  async function confirmBinding() {
    if (!editingShortcutId || !capturedBinding || busy || captureError) return;

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

    <Button
      variant="outline"
      size="sm"
      disabled={busy || !hasAnyCustomBindings(shortcutGroups)}
      onclick={handleResetAll}
    >
      <RotateCcw size={14} />
      Reset all
    </Button>
  </div>

  <div class="panel-body">
    {#if loading}
      <div class="empty-state">Loading keyboard shortcuts…</div>
    {:else if shortcutGroups.length === 0}
      <div class="empty-state">No keyboard shortcuts are currently registered.</div>
    {:else}
      {#if error}
        <Alert.Root variant="destructive">
          <AlertCircle />
          <Alert.Description>{error}</Alert.Description>
        </Alert.Root>
      {/if}

      <div class="capture-hint">
        Press a new key combo while editing. Use <strong>Enter</strong> to save and
        <strong>Escape</strong> to cancel.
      </div>

      {#if captureError}
        <Alert.Root variant="destructive">
          <AlertCircle />
          <Alert.Description>{captureError}</Alert.Description>
        </Alert.Root>
      {/if}

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
                    <Badge variant="outline">Custom</Badge>
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
                  <Button
                    variant="outline"
                    size="xs"
                    onclick={confirmBinding}
                    disabled={busy || !capturedBinding || !!conflictId}
                  >
                    <Save size={12} />
                    Save
                  </Button>
                  <Button variant="ghost" size="xs" onclick={cancelEditing} disabled={busy}>
                    <X size={12} />
                    Cancel
                  </Button>
                {:else}
                  <Button
                    variant="ghost"
                    size="xs"
                    onclick={() => startEditing(shortcut.id)}
                    disabled={busy}
                  >
                    <Pencil size={12} />
                    Edit
                  </Button>
                  <Button
                    variant="ghost"
                    size="xs"
                    onclick={() => handleResetShortcut(shortcut.id)}
                    disabled={busy || !customized}
                  >
                    <RotateCcw size={12} />
                    Reset
                  </Button>
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

  .shortcut-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
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
