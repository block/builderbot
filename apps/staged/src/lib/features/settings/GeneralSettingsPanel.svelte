<script lang="ts">
  import { Settings2 } from 'lucide-svelte';
  import {
    preferences,
    getAvailableSyntaxThemes,
    selectSyntaxTheme,
    setAutoReviewMode,
    type AutoReviewMode,
  } from './preferences.svelte';
  import FormToggle from '../../shared/FormToggle.svelte';

  const autoReviewOptions: { value: AutoReviewMode; label: string }[] = [
    { value: 'never', label: 'Never' },
    { value: 'after-changes', label: 'After changes' },
  ];

  let autoReviewValue = $state(preferences.autoReviewMode);

  // Sync from store on load
  $effect(() => {
    autoReviewValue = preferences.autoReviewMode;
  });

  // Persist when user changes
  $effect(() => {
    if (preferences.loaded && autoReviewValue !== preferences.autoReviewMode) {
      setAutoReviewMode(autoReviewValue);
    }
  });

  const themes = $derived(getAvailableSyntaxThemes());

  function handleThemeChange(e: Event) {
    const select = e.target as HTMLSelectElement;
    selectSyntaxTheme(select.value);
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
      <span class="field-label">Auto start code reviews</span>
      <FormToggle options={autoReviewOptions} bind:value={autoReviewValue} />
      <p class="field-description">
        {#if autoReviewValue === 'after-changes'}
          A code review will automatically start after each commit session completes.
        {:else}
          Code reviews will only start when you manually request them.
        {/if}
      </p>
    </div>

    <div class="field">
      <label class="field-label" for="theme-select">Theme</label>
      <select
        id="theme-select"
        class="theme-select"
        value={preferences.syntaxTheme}
        onchange={handleThemeChange}
      >
        {#each themes as theme (theme.name)}
          <option value={theme.name}>{theme.name}</option>
        {/each}
      </select>
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
</style>
