<script lang="ts">
  import { onMount } from 'svelte';
  import { Settings2, Info, Check, ChevronDown } from 'lucide-svelte';
  import {
    preferences,
    getAvailableSyntaxThemes,
    selectSyntaxTheme,
    setAutoReviewMode,
    loadAllThemePreviewColors,
    isLightTheme,
    type AutoReviewMode,
    type ThemePreviewColors,
  } from './preferences.svelte';

  const autoReviewOptions: { value: AutoReviewMode; label: string }[] = [
    { value: 'never', label: 'Never' },
    { value: 'after-changes', label: 'After changes' },
  ];

  type ThemeFilter = 'all' | 'light' | 'dark';

  const allThemes = $derived(getAvailableSyntaxThemes());

  let themeFilter = $state<ThemeFilter>('all');
  let previewColors = $state<Map<string, ThemePreviewColors>>(new Map());
  let dropdownOpen = $state(false);

  const themes = $derived(
    themeFilter === 'all'
      ? allThemes
      : allThemes.filter((t) =>
          themeFilter === 'light' ? isLightTheme(t.name) : !isLightTheme(t.name)
        )
  );
  let dropdownRef = $state<HTMLDivElement | null>(null);

  const activeColors = $derived(previewColors.get(preferences.syntaxTheme));

  onMount(() => {
    loadAllThemePreviewColors().then((colors) => {
      previewColors = colors;
    });

    function handleClickOutside(e: MouseEvent) {
      if (dropdownRef && !dropdownRef.contains(e.target as Node)) {
        dropdownOpen = false;
      }
    }
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  });

  function handleAutoReviewChange(e: Event) {
    const select = e.target as HTMLSelectElement;
    setAutoReviewMode(select.value as AutoReviewMode);
  }

  function handleThemeSelect(name: string) {
    selectSyntaxTheme(name);
    dropdownOpen = false;
  }
</script>

<div class="general-settings-panel">
  <div class="panel-intro">
    <div class="intro-copy">
      <h2>
        <Settings2 size={16} />
        General
      </h2>
      <p>General preferences.</p>
    </div>
  </div>

  <div class="panel-body">
    <div class="field">
      <label class="field-label" for="auto-review-select">Auto start code reviews</label>
      <select
        id="auto-review-select"
        class="theme-select"
        value={preferences.autoReviewMode}
        onchange={handleAutoReviewChange}
      >
        {#each autoReviewOptions as opt (opt.value)}
          <option value={opt.value}>{opt.label}</option>
        {/each}
      </select>
      <p class="field-description">
        <Info size={12} />
        {#if preferences.autoReviewMode === 'after-changes'}
          A code review will automatically start after each commit session completes.
        {:else}
          Code reviews will only start when you manually request them.
        {/if}
      </p>
    </div>

    <div class="field">
      <span class="field-label">Theme</span>
      <div class="theme-dropdown" bind:this={dropdownRef}>
        <button class="theme-dropdown-trigger" onclick={() => (dropdownOpen = !dropdownOpen)}>
          <span class="trigger-swatch" style:background={activeColors?.bg ?? 'var(--bg-primary)'}>
            <span style:color={activeColors?.fg ?? 'var(--text-primary)'}
              >{preferences.syntaxTheme}</span
            >
          </span>
          <ChevronDown size={14} />
        </button>

        {#if dropdownOpen}
          <div class="theme-dropdown-panel">
            <div class="theme-filters">
              {#each ['all', 'light', 'dark'] as filter (filter)}
                <button
                  class="theme-filter-btn"
                  class:active={themeFilter === filter}
                  onclick={() => (themeFilter = filter as ThemeFilter)}
                >
                  {filter.charAt(0).toUpperCase() + filter.slice(1)}
                </button>
              {/each}
            </div>
            {#each themes as theme (theme.name)}
              {@const colors = previewColors.get(theme.name)}
              {@const isActive = preferences.syntaxTheme === theme.name}
              <button
                class="theme-swatch"
                class:active={isActive}
                style:background={colors?.bg ?? 'var(--bg-primary)'}
                style:border-color={isActive
                  ? (colors?.comment ?? 'var(--border-emphasis)')
                  : 'transparent'}
                onclick={() => handleThemeSelect(theme.name)}
              >
                <span class="swatch-name" style:color={colors?.fg ?? 'var(--text-primary)'}
                  >{theme.name}</span
                >
                {#if isActive}
                  <span class="swatch-check" style:color={colors?.comment ?? 'var(--text-muted)'}>
                    <Check size={14} />
                  </span>
                {/if}
              </button>
            {/each}
          </div>
        {/if}
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
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .field-description {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.4;
    display: flex;
    align-items: baseline;
    gap: 4px;
  }

  .field-label {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  .theme-select {
    width: 100%;
    max-width: 320px;
    padding: 6px 10px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 5px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      border-color 0.1s,
      background-color 0.1s;
  }

  .theme-select:focus {
    outline: none;
    border-color: var(--border-emphasis);
    background-color: var(--bg-hover);
  }

  .theme-dropdown {
    position: relative;
    max-width: 320px;
  }

  .theme-dropdown-trigger {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 0;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 5px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    cursor: pointer;
    overflow: hidden;
    transition:
      border-color 0.1s,
      background-color 0.1s;
  }

  .theme-dropdown-trigger:hover {
    border-color: var(--border-emphasis);
  }

  .trigger-swatch {
    flex: 1;
    padding: 6px 10px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    border-radius: 4px 0 0 4px;
  }

  .theme-dropdown-trigger :global(svg) {
    flex-shrink: 0;
    margin-right: 8px;
    color: var(--text-muted);
  }

  .theme-dropdown-panel {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    max-height: 320px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 4px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 5px;
    box-shadow: var(--shadow-elevated);
    z-index: 10;
  }

  .theme-filters {
    display: flex;
    gap: 2px;
    padding: 0 0 4px;
    border-bottom: 1px solid var(--border-subtle);
    margin-bottom: 2px;
  }

  .theme-filter-btn {
    flex: 1;
    padding: 4px 8px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-family: inherit;
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .theme-filter-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .theme-filter-btn.active {
    background: var(--bg-active);
    color: var(--text-primary);
    font-weight: 600;
  }

  .theme-swatch {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 8px;
    border: 1.5px solid transparent;
    border-radius: 4px;
    cursor: pointer;
    font-family: inherit;
    transition: border-color 0.1s;
  }

  .theme-swatch:hover {
    filter: brightness(1.08);
  }

  .theme-swatch.active {
    border-style: solid;
  }

  .swatch-name {
    font-size: var(--size-xs);
    font-weight: 500;
    white-space: nowrap;
  }

  .swatch-check {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    margin-left: auto;
  }
</style>
