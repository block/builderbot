<!--
  ThemeSelectorModal.svelte - Theme picker dropdown

  Shows all available syntax themes with search, keyboard navigation,
  and light/dark indicators. Selecting a theme applies it immediately
  via the adaptive theme system.
-->
<script lang="ts">
  import { Search, Sun, Moon } from 'lucide-svelte';
  import {
    preferences,
    getAvailableSyntaxThemes,
    selectSyntaxTheme,
    isLightTheme,
  } from './preferences.svelte';

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  let searchQuery = $state('');
  let selectedIndex = $state(-1);
  let searchInputRef = $state<HTMLInputElement | null>(null);
  let dropdownRef = $state<HTMLDivElement | null>(null);

  // Focus search input on mount
  $effect(() => {
    searchInputRef?.focus();
  });

  // Filter themes based on search
  let filteredThemes = $derived.by(() => {
    const themes = getAvailableSyntaxThemes();
    if (!searchQuery.trim()) return themes;
    const query = searchQuery.toLowerCase();
    return themes.filter((t) => t.name.toLowerCase().includes(query));
  });

  // Reset selection when filter changes
  $effect(() => {
    const _ = filteredThemes;
    selectedIndex = -1;
  });

  function handleThemeSelect(themeName: string) {
    selectSyntaxTheme(themeName);
    onClose();
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      onClose();
      event.preventDefault();
    } else if (event.key === 'Enter') {
      if (
        filteredThemes.length > 0 &&
        selectedIndex >= 0 &&
        selectedIndex < filteredThemes.length
      ) {
        handleThemeSelect(filteredThemes[selectedIndex].name);
        event.preventDefault();
      }
    } else if (event.key === 'ArrowDown') {
      event.preventDefault();
      selectedIndex = Math.min(selectedIndex + 1, filteredThemes.length - 1);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      if (selectedIndex > 0) {
        selectedIndex = selectedIndex - 1;
      }
    }
  }

  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (dropdownRef && !dropdownRef.contains(target) && !target.closest('.theme-btn')) {
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} onclick={handleClickOutside} />

<div class="theme-dropdown" bind:this={dropdownRef}>
  <div class="search-container">
    <Search size={14} class="search-icon" />
    <input
      bind:this={searchInputRef}
      type="text"
      class="search-input"
      placeholder="Search themes..."
      bind:value={searchQuery}
      autocomplete="off"
      spellcheck="false"
    />
  </div>

  <div class="theme-list">
    {#each filteredThemes as theme, i (theme.name)}
      <button
        class="theme-item"
        class:active={theme.name === preferences.syntaxTheme}
        class:selected={i === selectedIndex}
        onclick={() => handleThemeSelect(theme.name)}
      >
        <span class="theme-indicator">
          {#if isLightTheme(theme.name)}
            <Sun size={12} />
          {:else}
            <Moon size={12} />
          {/if}
        </span>
        <span class="theme-name">{theme.name}</span>
      </button>
    {:else}
      <div class="no-results">No themes match "{searchQuery}"</div>
    {/each}
  </div>
</div>

<style>
  .theme-dropdown {
    position: fixed;
    top: 40px;
    right: 8px;
    z-index: 1000;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    width: 260px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .search-container {
    position: relative;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .search-container :global(.search-icon) {
    position: absolute;
    left: 20px;
    top: 50%;
    transform: translateY(-50%);
    color: var(--text-faint);
    pointer-events: none;
  }

  .search-input {
    width: 100%;
    padding: 6px 8px 6px 30px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 5px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    box-sizing: border-box;
    transition:
      border-color 0.1s,
      background-color 0.1s;
  }

  .search-input::placeholder {
    color: var(--text-faint);
  }

  .search-input:focus {
    outline: none;
    border-color: var(--border-emphasis);
    background-color: var(--bg-hover);
  }

  .theme-list {
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    max-height: 320px;
    padding: 4px 0;
  }

  .theme-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 12px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-xs);
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .theme-item:hover,
  .theme-item.selected {
    background-color: var(--bg-hover);
  }

  .theme-item.active {
    background-color: var(--ui-selection);
    border-left: 2px solid var(--ui-accent);
    padding-left: 10px;
  }

  .theme-item.active .theme-name {
    color: var(--ui-accent);
    font-weight: 500;
  }

  .theme-item.active .theme-indicator {
    color: var(--ui-accent);
  }

  .theme-indicator {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .theme-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .no-results {
    padding: 16px 12px;
    text-align: center;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }
</style>
