<!--
  KeyboardShortcutsModal.svelte - Displays available keyboard shortcuts
  
  Shows a categorized list of all keyboard shortcuts available in the app.
  Pulls from the central keyboard registry so it's always up to date.
-->
<script lang="ts">
  import { Keyboard } from 'lucide-svelte';
  import {
    getAllShortcuts,
    formatShortcutKeys,
    isMac,
    type Shortcut,
    type FormattedKey,
  } from './services/keyboard';

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  let dropdownRef = $state<HTMLDivElement | null>(null);

  // Category display names and order
  const categoryInfo: Record<string, { title: string; order: number }> = {
    navigation: { title: 'Navigation', order: 1 },
    view: { title: 'View', order: 2 },
    comments: { title: 'Comments', order: 3 },
    files: { title: 'Files', order: 4 },
  };

  interface DisplayShortcut {
    description: string;
    keys: FormattedKey[];
  }

  interface ShortcutGroup {
    title: string;
    order: number;
    shortcuts: DisplayShortcut[];
  }

  // Create a unique string key for deduplication
  function keyToString(k: FormattedKey): string {
    return k.modifiers.join('') + k.key;
  }

  // Menu shortcuts handled by Tauri (not in keyboard service, but should show here)
  function getMenuShortcuts(): DisplayShortcut[] {
    const cmdKey = isMac() ? '⌘' : 'Ctrl';
    return [
      { description: 'New tab', keys: [{ modifiers: [cmdKey], key: 'T' }] },
      { description: 'Close tab', keys: [{ modifiers: [cmdKey], key: 'W' }] },
    ];
  }

  // Get shortcuts grouped by category, with duplicate descriptions merged
  function getGroupedShortcuts(): ShortcutGroup[] {
    const allShortcuts = getAllShortcuts();
    const byCategory = new Map<string, Shortcut[]>();

    // Group by category
    for (const shortcut of allShortcuts) {
      const list = byCategory.get(shortcut.category) || [];
      list.push(shortcut);
      byCategory.set(shortcut.category, list);
    }

    const groups: ShortcutGroup[] = [];

    for (const [category, shortcuts] of byCategory) {
      const info = categoryInfo[category] || { title: category, order: 99 };

      // Merge shortcuts with same description (e.g., j and ArrowDown both do "Next diff hunk")
      const byDescription = new Map<string, FormattedKey[]>();
      for (const shortcut of shortcuts) {
        const keys = byDescription.get(shortcut.description) || [];
        keys.push(...formatShortcutKeys(shortcut));
        byDescription.set(shortcut.description, keys);
      }

      const displayShortcuts: DisplayShortcut[] = [];
      for (const [description, keys] of byDescription) {
        // Deduplicate keys
        const seen = new Set<string>();
        const uniqueKeys = keys.filter((k) => {
          const str = keyToString(k);
          if (seen.has(str)) return false;
          seen.add(str);
          return true;
        });
        displayShortcuts.push({ description, keys: uniqueKeys });
      }

      // Add menu shortcuts to the 'view' category
      if (category === 'view') {
        displayShortcuts.push(...getMenuShortcuts());
      }

      groups.push({
        title: info.title,
        order: info.order,
        shortcuts: displayShortcuts,
      });
    }

    return groups.sort((a, b) => a.order - b.order);
  }

  let groups = $derived(getGroupedShortcuts());

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      onClose();
      event.preventDefault();
    }
  }

  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (dropdownRef && !dropdownRef.contains(target) && !target.closest('.shortcuts-btn')) {
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} onclick={handleClickOutside} />

<div class="shortcuts-dropdown" bind:this={dropdownRef}>
  <div class="shortcuts-header">
    <Keyboard size={14} />
    <span>Keyboard Shortcuts</span>
  </div>

  <div class="shortcuts-content">
    {#each groups as group}
      <div class="shortcut-group">
        <div class="group-title">{group.title}</div>
        {#each group.shortcuts as shortcut}
          <div class="shortcut-row">
            <span class="shortcut-description">{shortcut.description}</span>
            <span class="shortcut-keys">
              {#each shortcut.keys as key, i}
                {#if i > 0}<span class="key-separator">/</span>{/if}
                <span class="key-combo">{key.modifiers.join('')}{key.key}</span>
              {/each}
            </span>
          </div>
        {/each}
      </div>
    {/each}
  </div>
</div>

<style>
  .shortcuts-dropdown {
    position: fixed;
    top: 40px;
    right: 8px;
    z-index: 1000;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    width: 280px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .shortcuts-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border-subtle);
    color: var(--text-primary);
    font-size: var(--size-xs);
    font-weight: 500;
  }

  .shortcuts-header :global(svg) {
    color: var(--text-muted);
  }

  .shortcuts-content {
    display: flex;
    flex-direction: column;
    padding: 8px 0;
    max-height: 400px;
    overflow-y: auto;
  }

  .shortcut-group {
    padding: 4px 12px 8px;
  }

  .shortcut-group:not(:last-child) {
    border-bottom: 1px solid var(--border-subtle);
    margin-bottom: 4px;
  }

  .group-title {
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin-bottom: 6px;
  }

  .shortcut-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 0;
    gap: 12px;
  }

  .shortcut-description {
    font-size: var(--size-xs);
    color: var(--text-primary);
    white-space: nowrap;
  }

  .shortcut-keys {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .key-separator {
    color: var(--text-faint);
    font-size: var(--size-xs);
    margin: 0 2px;
  }

  .key-combo {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }
</style>
