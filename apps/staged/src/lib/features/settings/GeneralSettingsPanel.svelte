<script lang="ts">
  import { onMount } from 'svelte';
  import { Settings2, Info, Check } from 'lucide-svelte';
  import {
    preferences,
    getAvailableSyntaxThemes,
    selectSyntaxTheme,
    setAutoReviewMode,
    loadAllThemePreviewColors,
    type AutoReviewMode,
    type ThemePreviewColors,
  } from './preferences.svelte';

  const autoReviewOptions: { value: AutoReviewMode; label: string }[] = [
    { value: 'never', label: 'Never' },
    { value: 'after-changes', label: 'After changes' },
  ];

  const themes = $derived(getAvailableSyntaxThemes());

  let previewColors = $state<Map<string, ThemePreviewColors>>(new Map());

  onMount(() => {
    loadAllThemePreviewColors().then((colors) => {
      previewColors = colors;
    });
  });

  function handleAutoReviewChange(e: Event) {
    const select = e.target as HTMLSelectElement;
    setAutoReviewMode(select.value as AutoReviewMode);
  }

  function handleThemeSelect(name: string) {
    selectSyntaxTheme(name);
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
      <div class="theme-list">
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
            <span class="swatch-comment" style:color={colors?.comment ?? 'var(--text-muted)'}
              >// preview</span
            >
            {#if isActive}
              <span class="swatch-check" style:color={colors?.comment ?? 'var(--text-muted)'}>
                <Check size={14} />
              </span>
            {/if}
          </button>
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

  .theme-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-width: 360px;
  }

  .theme-swatch {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    border: 1.5px solid transparent;
    border-radius: 5px;
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

  .swatch-comment {
    font-size: var(--size-xs);
    opacity: 0.8;
    white-space: nowrap;
    margin-left: auto;
  }

  .swatch-check {
    display: flex;
    align-items: center;
    flex-shrink: 0;
  }
</style>
