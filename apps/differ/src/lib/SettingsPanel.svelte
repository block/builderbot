<!--
  SettingsPanel — Settings dropdown for the titlebar.

  Provides controls for code font family, code font size, and UI font size.
  All changes apply immediately via the preferences store.
-->
<script lang="ts">
  import { Settings, Type, Minus, Plus, Search } from '@lucide/svelte';
  import {
    preferences,
    setCodeFontFamily,
    setCodeFontSize,
    setUiFontSize,
  } from './preferences.svelte';
  import { listSystemFonts } from './commands';

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  let panelRef = $state<HTMLDivElement | null>(null);

  // Popular coding fonts to pin at the top (matched case-insensitively against system fonts)
  const POPULAR_CODING_FONTS = [
    'SF Mono',
    'Menlo',
    'JetBrains Mono',
    'Fira Code',
    'Cascadia Code',
    'Source Code Pro',
    'Inconsolata',
    'IBM Plex Mono',
    'Hack',
    'Monaco',
    'Courier New',
  ];

  let systemFonts = $state<string[]>([]);
  let fontSearch = $state('');
  let showFontList = $state(false);
  let fontListRef = $state<HTMLDivElement | null>(null);

  // Load system fonts on mount
  $effect(() => {
    listSystemFonts().then((fonts) => {
      systemFonts = fonts;
    });
  });

  // Current font display name (strip quotes and fallbacks for display)
  let currentFontDisplay = $derived.by(() => {
    return preferences.codeFontFamily;
  });

  // Split fonts into popular (installed) and the rest
  let popularInstalled = $derived(
    POPULAR_CODING_FONTS.filter((name) =>
      systemFonts.some((f) => f.toLowerCase() === name.toLowerCase())
    )
  );

  let otherFonts = $derived(
    systemFonts.filter(
      (f) => !POPULAR_CODING_FONTS.some((p) => p.toLowerCase() === f.toLowerCase())
    )
  );

  let filteredPopular = $derived(
    fontSearch.trim()
      ? popularInstalled.filter((f) => f.toLowerCase().includes(fontSearch.toLowerCase()))
      : popularInstalled
  );

  let filteredOther = $derived(
    fontSearch.trim()
      ? otherFonts.filter((f) => f.toLowerCase().includes(fontSearch.toLowerCase()))
      : otherFonts
  );

  function selectFont(name: string) {
    setCodeFontFamily(name);
    showFontList = false;
    fontSearch = '';
  }

  function isActiveFont(name: string): boolean {
    return preferences.codeFontFamily.toLowerCase() === name.toLowerCase();
  }

  function adjustCodeFontSize(delta: number) {
    const next = Math.min(20, Math.max(10, preferences.codeFontSize + delta));
    if (next !== preferences.codeFontSize) {
      setCodeFontSize(next);
    }
  }

  function adjustUiFontSize(delta: number) {
    const next = Math.min(18, Math.max(10, preferences.uiFontSize + delta));
    if (next !== preferences.uiFontSize) {
      setUiFontSize(next);
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      if (showFontList) {
        showFontList = false;
        fontSearch = '';
        event.preventDefault();
        event.stopPropagation();
      } else {
        onClose();
        event.preventDefault();
        event.stopPropagation();
      }
    }
  }

  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (panelRef && !panelRef.contains(target) && !target.closest('.settings-btn')) {
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} onclick={handleClickOutside} />

<div class="settings-panel" bind:this={panelRef}>
  <div class="panel-header">
    <Settings size={14} />
    <span>Settings</span>
  </div>

  <!-- Code Font Family -->
  <div class="section">
    <div class="section-label">
      <Type size={13} />
      <span>Code Font</span>
    </div>
    <button class="font-picker-btn" onclick={() => (showFontList = !showFontList)}>
      <span class="font-picker-name" style:font-family="'{currentFontDisplay}', monospace"
        >{currentFontDisplay}</span
      >
    </button>
    {#if showFontList}
      <div class="font-picker-dropdown" bind:this={fontListRef}>
        <div class="font-search-container">
          <Search size={13} class="font-search-icon" />
          <input
            type="text"
            class="font-search-input"
            placeholder="Search fonts..."
            bind:value={fontSearch}
            autocomplete="off"
            spellcheck="false"
          />
        </div>
        <div class="font-list">
          {#if filteredPopular.length > 0}
            <div class="font-group-label">Coding Fonts</div>
            {#each filteredPopular as font (font)}
              <button
                class="font-item"
                class:active={isActiveFont(font)}
                style:font-family="'{font}', monospace"
                onclick={() => selectFont(font)}
              >
                {font}
              </button>
            {/each}
          {/if}
          {#if filteredOther.length > 0}
            {#if filteredPopular.length > 0}
              <div class="font-group-label">All Fonts</div>
            {/if}
            {#each filteredOther as font (font)}
              <button
                class="font-item"
                class:active={isActiveFont(font)}
                style:font-family="'{font}'"
                onclick={() => selectFont(font)}
              >
                {font}
              </button>
            {/each}
          {/if}
          {#if filteredPopular.length === 0 && filteredOther.length === 0}
            <div class="no-results">No fonts match "{fontSearch}"</div>
          {/if}
        </div>
      </div>
    {/if}
  </div>

  <!-- Code Font Size -->
  <div class="section">
    <div class="section-label">
      <Type size={13} />
      <span>Code Font Size</span>
    </div>
    <div class="stepper">
      <button
        class="stepper-btn"
        onclick={() => adjustCodeFontSize(-1)}
        disabled={preferences.codeFontSize <= 10}
        aria-label="Decrease code font size"
      >
        <Minus size={12} />
      </button>
      <span class="stepper-value">{preferences.codeFontSize}px</span>
      <button
        class="stepper-btn"
        onclick={() => adjustCodeFontSize(1)}
        disabled={preferences.codeFontSize >= 20}
        aria-label="Increase code font size"
      >
        <Plus size={12} />
      </button>
    </div>
  </div>

  <!-- UI Font Size -->
  <div class="section">
    <div class="section-label">
      <Type size={13} />
      <span>UI Font Size</span>
    </div>
    <div class="stepper">
      <button
        class="stepper-btn"
        onclick={() => adjustUiFontSize(-1)}
        disabled={preferences.uiFontSize <= 10}
        aria-label="Decrease UI font size"
      >
        <Minus size={12} />
      </button>
      <span class="stepper-value">{preferences.uiFontSize}px</span>
      <button
        class="stepper-btn"
        onclick={() => adjustUiFontSize(1)}
        disabled={preferences.uiFontSize >= 18}
        aria-label="Increase UI font size"
      >
        <Plus size={12} />
      </button>
    </div>
  </div>
</div>

<style>
  .settings-panel {
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

  .panel-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 10px 12px;
    font-size: var(--size-xs);
    font-weight: 500;
    color: var(--text-primary);
    border-bottom: 1px solid var(--border-subtle);
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .section:last-child {
    border-bottom: none;
  }

  .section-label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .font-picker-btn {
    display: flex;
    align-items: center;
    width: 100%;
    padding: 6px 8px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 5px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    cursor: pointer;
    text-align: left;
    transition:
      border-color 0.1s,
      background-color 0.1s;
  }

  .font-picker-btn:hover {
    border-color: var(--border-emphasis);
    background-color: var(--bg-hover);
  }

  .font-picker-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .font-picker-dropdown {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    background: var(--bg-primary);
    overflow: hidden;
  }

  .font-search-container {
    position: relative;
    padding: 6px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .font-search-container :global(.font-search-icon) {
    position: absolute;
    left: 14px;
    top: 50%;
    transform: translateY(-50%);
    color: var(--text-faint);
    pointer-events: none;
  }

  .font-search-input {
    width: 100%;
    padding: 5px 8px 5px 26px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    border-radius: 4px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    font-family: inherit;
    box-sizing: border-box;
  }

  .font-search-input::placeholder {
    color: var(--text-faint);
  }

  .font-search-input:focus {
    outline: none;
    border-color: var(--border-emphasis);
  }

  .font-list {
    max-height: 240px;
    overflow-y: auto;
    padding: 2px 0;
  }

  .font-group-label {
    padding: 5px 10px 3px;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.5px;
    color: var(--text-faint);
    text-transform: uppercase;
  }

  .font-item {
    display: block;
    width: 100%;
    padding: 5px 10px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-xs);
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .font-item:hover {
    background-color: var(--bg-hover);
  }

  .font-item.active {
    background-color: var(--ui-selection);
    border-left: 2px solid var(--text-primary);
    padding-left: 8px;
    font-weight: 500;
  }

  .no-results {
    padding: 12px 10px;
    text-align: center;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .stepper {
    display: flex;
    align-items: center;
    gap: 0;
    border: 1px solid var(--border-muted);
    border-radius: 5px;
    overflow: hidden;
    width: fit-content;
  }

  .stepper-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    background: var(--bg-primary);
    border: none;
    color: var(--text-primary);
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .stepper-btn:hover:not(:disabled) {
    background-color: var(--bg-hover);
  }

  .stepper-btn:disabled {
    color: var(--text-faint);
    cursor: default;
  }

  .stepper-value {
    display: flex;
    align-items: center;
    justify-content: center;
    min-width: 44px;
    height: 28px;
    padding: 0 4px;
    font-size: var(--size-xs);
    color: var(--text-primary);
    background: var(--bg-primary);
    border-left: 1px solid var(--border-muted);
    border-right: 1px solid var(--border-muted);
    font-variant-numeric: tabular-nums;
  }
</style>
