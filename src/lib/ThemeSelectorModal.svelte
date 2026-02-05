<script lang="ts">
  import { onMount } from 'svelte';
  import { Search, Sun, Moon, Upload, Check, AlertCircle, FolderOpen } from 'lucide-svelte';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import {
    preferences,
    getAvailableSyntaxThemes,
    selectSyntaxTheme,
    isLightTheme,
    loadCustomThemes,
  } from './stores/preferences.svelte';
  import {
    validateTheme,
    installTheme,
    pickThemeFile,
    readJsonFile,
    type ThemeValidation,
  } from './services/customThemes';

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  let searchQuery = $state('');
  let selectedIndex = $state(-1); // -1 means no keyboard selection
  let searchInputRef = $state<HTMLInputElement | null>(null);
  let dropdownRef = $state<HTMLDivElement | null>(null);

  // Drop zone state
  let showDropZone = $state(false);
  let isDragging = $state(false);
  let isProcessing = $state(false);
  let validationResult = $state<ThemeValidation | null>(null);
  let pendingFile = $state<{ content: string; filename: string } | null>(null);
  let installError = $state<string | null>(null);

  // Focus search input on mount (only when not showing drop zone)
  $effect(() => {
    if (!showDropZone) {
      searchInputRef?.focus();
    }
  });

  // Listen for Tauri drag-drop events when drop zone is shown
  onMount(() => {
    let unlisten: (() => void) | null = null;

    const setupListener = async () => {
      const webview = getCurrentWebview();
      unlisten = await webview.onDragDropEvent((event) => {
        if (!showDropZone) return;

        if (event.payload.type === 'enter' || event.payload.type === 'over') {
          isDragging = true;
        } else if (event.payload.type === 'drop') {
          isDragging = false;
          const paths = event.payload.paths;
          if (paths.length > 0) {
            handleFileDrop(paths[0]);
          }
        } else if (event.payload.type === 'leave') {
          isDragging = false;
        }
      });
    };

    setupListener();

    return () => {
      if (unlisten) unlisten();
    };
  });

  async function handleFileDrop(path: string) {
    if (!path.toLowerCase().endsWith('.json')) {
      validationResult = {
        valid: false,
        name: null,
        is_light: null,
        error: 'Please drop a .json file',
      };
      return;
    }

    isProcessing = true;
    validationResult = null;
    installError = null;

    try {
      const content = await readJsonFile(path);
      const validation = await validateTheme(content);
      validationResult = validation;

      if (validation.valid) {
        const filename = path.split('/').pop() || path.split('\\').pop() || 'theme.json';
        pendingFile = { content, filename };
      }
    } catch (e) {
      validationResult = {
        valid: false,
        name: null,
        is_light: null,
        error: `Failed to read file: ${e}`,
      };
    } finally {
      isProcessing = false;
    }
  }

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
      if (showDropZone) {
        resetDropZone();
      } else {
        onClose();
      }
      event.preventDefault();
    } else if (!showDropZone) {
      if (event.key === 'Enter') {
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
  }

  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    // Close if click is outside the dropdown and not on the theme button
    if (dropdownRef && !dropdownRef.contains(target) && !target.closest('.theme-btn')) {
      onClose();
    }
  }

  // Drop zone handlers
  function handleDragEnter(event: DragEvent) {
    event.preventDefault();
    isDragging = true;
  }

  function handleDragOver(event: DragEvent) {
    event.preventDefault();
    isDragging = true;
  }

  function handleDragLeave(event: DragEvent) {
    event.preventDefault();
    // Only set to false if we're leaving the drop zone entirely
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const x = event.clientX;
    const y = event.clientY;
    if (x < rect.left || x > rect.right || y < rect.top || y > rect.bottom) {
      isDragging = false;
    }
  }

  async function handleDrop(event: DragEvent) {
    event.preventDefault();
    isDragging = false;

    const files = event.dataTransfer?.files;
    if (!files || files.length === 0) return;

    const file = files[0];
    if (!file.name.toLowerCase().endsWith('.json')) {
      validationResult = {
        valid: false,
        name: null,
        is_light: null,
        error: 'Please drop a .json file',
      };
      return;
    }

    await processFile(file);
  }

  async function processFile(file: File) {
    isProcessing = true;
    validationResult = null;
    installError = null;

    try {
      const content = await file.text();
      const validation = await validateTheme(content);
      validationResult = validation;

      if (validation.valid) {
        pendingFile = { content, filename: file.name };
      }
    } catch (e) {
      validationResult = {
        valid: false,
        name: null,
        is_light: null,
        error: `Failed to read file: ${e}`,
      };
    } finally {
      isProcessing = false;
    }
  }

  async function handleFilePick() {
    try {
      const path = await pickThemeFile();
      if (!path) return;

      isProcessing = true;
      validationResult = null;
      installError = null;

      const content = await readJsonFile(path);
      const validation = await validateTheme(content);
      validationResult = validation;

      if (validation.valid) {
        // Extract filename from path
        const filename = path.split('/').pop() || path.split('\\').pop() || 'theme.json';
        pendingFile = { content, filename };
      }
    } catch (e) {
      validationResult = {
        valid: false,
        name: null,
        is_light: null,
        error: `Failed to read file: ${e}`,
      };
    } finally {
      isProcessing = false;
    }
  }

  async function handleInstall() {
    if (!pendingFile || !validationResult?.valid) return;

    isProcessing = true;
    installError = null;

    try {
      const installed = await installTheme(pendingFile.content, pendingFile.filename);

      // Refresh the custom themes list
      await loadCustomThemes();

      // Select the newly installed theme
      await selectSyntaxTheme(installed.name);

      // Close the dropdown
      onClose();
    } catch (e) {
      installError = `${e}`;
    } finally {
      isProcessing = false;
    }
  }

  function resetDropZone() {
    showDropZone = false;
    isDragging = false;
    isProcessing = false;
    validationResult = null;
    pendingFile = null;
    installError = null;
  }
</script>

<svelte:window onkeydown={handleKeydown} onclick={handleClickOutside} />

<div class="theme-dropdown" bind:this={dropdownRef}>
  {#if showDropZone}
    <!-- Add Theme View -->
    <div class="add-theme-view">
      <div class="add-theme-header">
        <button class="back-btn" onclick={resetDropZone}>← Back</button>
        <span class="add-theme-title">Add Custom Theme</span>
      </div>

      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="drop-zone"
        class:dragging={isDragging}
        class:has-result={validationResult !== null}
        ondragenter={handleDragEnter}
        ondragover={handleDragOver}
        ondragleave={handleDragLeave}
        ondrop={handleDrop}
      >
        {#if isProcessing}
          <div class="drop-zone-content">
            <div class="spinner"></div>
            <span>Processing...</span>
          </div>
        {:else if validationResult}
          <div class="validation-result" class:valid={validationResult.valid}>
            {#if validationResult.valid}
              <Check size={24} />
              <span class="theme-name-preview">{validationResult.name || 'Unnamed Theme'}</span>
              <span class="theme-type-preview">
                {#if validationResult.is_light}
                  <Sun size={12} /> Light theme
                {:else}
                  <Moon size={12} /> Dark theme
                {/if}
              </span>
            {:else}
              <AlertCircle size={24} />
              <span class="error-message">{validationResult.error}</span>
            {/if}
          </div>
        {:else}
          <div class="drop-zone-content">
            <Upload size={24} />
            <span>Drop VS Code theme here</span>
            <span class="drop-zone-hint">or</span>
            <button class="browse-btn" onclick={handleFilePick}>
              <FolderOpen size={14} />
              Browse...
            </button>
          </div>
        {/if}
      </div>

      {#if installError}
        <div class="install-error">
          <AlertCircle size={12} />
          {installError}
        </div>
      {/if}

      {#if validationResult?.valid && pendingFile}
        <div class="install-actions">
          <button class="cancel-btn" onclick={resetDropZone}>Cancel</button>
          <button class="install-btn" onclick={handleInstall} disabled={isProcessing}>
            {isProcessing ? 'Installing...' : 'Install Theme'}
          </button>
        </div>
      {:else if validationResult && !validationResult.valid}
        <div class="install-actions">
          <button class="try-again-btn" onclick={() => (validationResult = null)}>
            Try Another File
          </button>
        </div>
      {/if}
    </div>
  {:else}
    <!-- Theme List View -->
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
          {#if theme.isCustom}
            <span class="theme-custom-badge">custom</span>
          {/if}
        </button>
      {:else}
        <div class="no-results">No themes match "{searchQuery}"</div>
      {/each}
    </div>

    <div class="dropdown-footer">
      <button
        class="add-custom-btn"
        onclick={(e) => {
          e.stopPropagation();
          showDropZone = true;
        }}
      >
        <Upload size={12} />
        <span>Add Custom Theme</span>
      </button>
    </div>
  {/if}
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
    background-color: var(--bg-primary);
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

  .theme-custom-badge {
    font-size: calc(var(--size-xs) - 2px);
    color: var(--text-faint);
    background: var(--bg-hover);
    padding: 1px 4px;
    border-radius: 3px;
    flex-shrink: 0;
  }

  .no-results {
    padding: 16px 12px;
    text-align: center;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .dropdown-footer {
    border-top: 1px solid var(--border-subtle);
    padding: 6px 10px;
  }

  .add-custom-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    width: 100%;
    padding: 6px 12px;
    background: none;
    border: none;
    border-radius: 5px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .add-custom-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .add-custom-btn :global(svg) {
    color: var(--text-faint);
  }

  /* Add Theme View */
  .add-theme-view {
    display: flex;
    flex-direction: column;
  }

  .add-theme-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .back-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    padding: 2px 4px;
    border-radius: 3px;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .back-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .add-theme-title {
    font-size: var(--size-xs);
    font-weight: 500;
    color: var(--text-primary);
  }

  .drop-zone {
    margin: 10px;
    padding: 24px 16px;
    border: 2px dashed var(--border-muted);
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    transition:
      border-color 0.15s,
      background-color 0.15s;
  }

  .drop-zone.dragging {
    border-color: var(--text-muted);
    background-color: var(--bg-hover);
  }

  .drop-zone.has-result {
    border-style: solid;
  }

  .drop-zone-content {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .drop-zone-content :global(svg) {
    color: var(--text-faint);
  }

  .drop-zone-hint {
    color: var(--text-faint);
    font-size: calc(var(--size-xs) - 1px);
  }

  .browse-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      background-color 0.1s,
      border-color 0.1s;
  }

  .browse-btn:hover {
    background: var(--bg-hover);
    border-color: var(--border-emphasis);
  }

  .validation-result {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    text-align: center;
  }

  .validation-result.valid {
    color: var(--status-added);
  }

  .validation-result.valid :global(svg) {
    color: var(--status-added);
  }

  .validation-result:not(.valid) {
    color: var(--status-deleted);
  }

  .validation-result:not(.valid) :global(svg) {
    color: var(--status-deleted);
  }

  .theme-name-preview {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .theme-type-preview {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .error-message {
    font-size: var(--size-xs);
    max-width: 200px;
  }

  .spinner {
    width: 20px;
    height: 20px;
    border: 2px solid var(--border-muted);
    border-top-color: var(--text-muted);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .install-error {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 10px;
    padding: 8px;
    background: color-mix(in srgb, var(--status-deleted) 10%, transparent);
    border-radius: 4px;
    color: var(--status-deleted);
    font-size: var(--size-xs);
  }

  .install-actions {
    display: flex;
    gap: 8px;
    padding: 10px;
    border-top: 1px solid var(--border-subtle);
  }

  .cancel-btn,
  .try-again-btn {
    flex: 1;
    padding: 6px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 5px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      background-color 0.1s,
      border-color 0.1s;
  }

  .cancel-btn:hover,
  .try-again-btn:hover {
    background: var(--bg-hover);
  }

  .install-btn {
    flex: 1;
    padding: 6px 12px;
    background: var(--status-added);
    border: none;
    border-radius: 5px;
    color: white;
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    transition:
      opacity 0.1s,
      background-color 0.1s;
  }

  .install-btn:hover:not(:disabled) {
    opacity: 0.9;
  }

  .install-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
