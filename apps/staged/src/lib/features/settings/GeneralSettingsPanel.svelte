<script lang="ts">
  import { Settings2, Sun, Moon } from 'lucide-svelte';
  import {
    preferences,
    getAvailableSyntaxThemes,
    selectSyntaxTheme,
    isLightTheme,
  } from './preferences.svelte';

  let searchQuery = $state('');

  let filteredThemes = $derived.by(() => {
    const themes = getAvailableSyntaxThemes();
    if (!searchQuery.trim()) return themes;
    const query = searchQuery.toLowerCase();
    return themes.filter((t) => t.name.toLowerCase().includes(query));
  });

  function handleThemeSelect(themeName: string) {
    selectSyntaxTheme(themeName);
  }
</script>

<div class="general-settings-panel">
  <div class="panel-intro">
    <div class="intro-copy">
      <h2>
        <Settings2 size={16} />
        General
      </h2>
      <p>Appearance and display preferences.</p>
    </div>
  </div>

  <div class="panel-body">
    <div class="field">
      <label class="field-label" for="theme-search">Theme</label>
      <input
        id="theme-search"
        type="text"
        class="theme-search-input"
        placeholder="Search themes..."
        bind:value={searchQuery}
        autocomplete="off"
        spellcheck="false"
      />
      <div class="theme-list">
        {#each filteredThemes as theme (theme.name)}
          <button
            class="theme-item"
            class:active={theme.name === preferences.syntaxTheme}
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
  </div>
</div>

<style>
  .general-settings-panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
    background: var(--bg-chrome);
  }

  .panel-intro {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 16px;
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
    font-size: var(--size-base);
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
    padding: 16px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .field-label {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  .theme-search-input {
    width: 100%;
    max-width: 320px;
    padding: 6px 10px;
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

  .theme-search-input::placeholder {
    color: var(--text-faint);
  }

  .theme-search-input:focus {
    outline: none;
    border-color: var(--border-emphasis);
    background-color: var(--bg-hover);
  }

  .theme-list {
    display: flex;
    flex-direction: column;
    max-height: 400px;
    overflow-y: auto;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    max-width: 320px;
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

  .theme-item:hover {
    background-color: var(--bg-hover);
  }

  .theme-item.active {
    background-color: var(--ui-selection);
    border-left: 2px solid var(--text-primary);
    padding-left: 10px;
  }

  .theme-item.active .theme-name {
    color: var(--text-primary);
    font-weight: 500;
  }

  .theme-item.active .theme-indicator {
    color: var(--text-primary);
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
