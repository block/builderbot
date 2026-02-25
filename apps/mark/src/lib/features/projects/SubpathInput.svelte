<!--
  SubpathInput.svelte - Subpath input with directory autocomplete and validation

  Features:
  - Debounced validation via validateSubpath command
  - Directory suggestions dropdown from listRepoDirectories
  - Spinner on RHS while validating
  - Error display below the field
  - Exposes waitForValidation() so parent can await pending checks
-->
<script lang="ts" module>
  export interface SubpathInputApi {
    waitForValidation(): Promise<boolean>;
    validating: boolean;
    validationError: string | null;
  }
</script>

<script lang="ts">
  import { FolderOpen } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import * as commands from '../../commands';

  interface Props {
    value: string;
    repo: string;
    disabled?: boolean;
    api?: SubpathInputApi;
  }

  let {
    value = $bindable(''),
    repo,
    disabled = false,
    api = $bindable(undefined),
  }: Props = $props();

  let validationError = $state<string | null>(null);
  let validating = $state(false);
  let suggestions = $state<string[]>([]);
  let showDropdown = $state(false);
  let highlightedIndex = $state(-1);
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  let inputEl: HTMLInputElement | undefined = $state();

  // Expose the API to the parent via bindable prop
  $effect(() => {
    api = {
      waitForValidation,
      get validating() {
        return validating;
      },
      get validationError() {
        return validationError;
      },
    };
  });

  function normalize(val: string): string {
    return val.trim().replace(/^\/+|\/+$/g, '');
  }

  // Split the value into parent path and current segment for suggestions
  function getParentPath(val: string): string {
    const trimmed = val.trim().replace(/^\/+/, '');
    const lastSlash = trimmed.lastIndexOf('/');
    if (lastSlash === -1) return '';
    return trimmed.substring(0, lastSlash);
  }

  function getCurrentSegment(val: string): string {
    const trimmed = val.trim().replace(/^\/+/, '');
    const lastSlash = trimmed.lastIndexOf('/');
    if (lastSlash === -1) return trimmed;
    return trimmed.substring(lastSlash + 1);
  }

  async function fetchSuggestions(path: string) {
    if (!repo) {
      suggestions = [];
      return;
    }

    try {
      const dirs = await commands.listRepoDirectories(repo, path);
      suggestions = dirs;
    } catch {
      suggestions = [];
    }
  }

  async function runValidation(trimmed: string): Promise<boolean> {
    if (!trimmed) {
      validationError = null;
      validating = false;
      return true;
    }

    validating = true;
    try {
      await commands.validateSubpath(repo, trimmed);
      validationError = null;
      return true;
    } catch (e) {
      validationError = typeof e === 'string' ? e : String(e);
      return false;
    } finally {
      validating = false;
    }
  }

  /**
   * Returns a promise that resolves to true if the current subpath is valid,
   * or false if validation fails. If a debounce is pending, fires immediately.
   */
  async function waitForValidation(): Promise<boolean> {
    // If there's a pending debounce, fire it immediately
    if (debounceTimer !== null) {
      clearTimeout(debounceTimer);
      debounceTimer = null;
    }

    const trimmed = normalize(value);
    return runValidation(trimmed);
  }

  function scheduleValidation() {
    if (debounceTimer !== null) {
      clearTimeout(debounceTimer);
    }

    const trimmed = normalize(value);
    if (!trimmed) {
      validationError = null;
      validating = false;
      return;
    }

    validating = true;
    debounceTimer = setTimeout(() => {
      debounceTimer = null;
      runValidation(trimmed);
    }, 500);
  }

  function handleInput() {
    highlightedIndex = -1;
    showDropdown = true;
    scheduleValidation();
    fetchSuggestions(getParentPath(value));
  }

  function handleFocus() {
    showDropdown = true;
    fetchSuggestions(getParentPath(value));
  }

  function handleBlur() {
    // Delay to allow click on dropdown items
    setTimeout(() => {
      showDropdown = false;
    }, 150);
  }

  function selectSuggestion(dir: string) {
    const parentPath = getParentPath(value);
    const newValue = parentPath ? `${parentPath}/${dir}` : dir;
    value = newValue;
    showDropdown = false;
    highlightedIndex = -1;
    scheduleValidation();
    // Fetch next level of suggestions
    fetchSuggestions(newValue);
    inputEl?.focus();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (!showDropdown || filteredSuggestions.length === 0) {
      return;
    }

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      highlightedIndex = Math.min(highlightedIndex + 1, filteredSuggestions.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      highlightedIndex = Math.max(highlightedIndex - 1, -1);
    } else if (e.key === 'Enter' && highlightedIndex >= 0) {
      e.preventDefault();
      e.stopPropagation();
      selectSuggestion(filteredSuggestions[highlightedIndex]);
    } else if (e.key === 'Escape') {
      showDropdown = false;
      highlightedIndex = -1;
    } else if (e.key === 'Tab' && highlightedIndex >= 0) {
      e.preventDefault();
      selectSuggestion(filteredSuggestions[highlightedIndex]);
    }
  }

  // Filter suggestions based on current segment being typed
  let filteredSuggestions = $derived.by(() => {
    const segment = getCurrentSegment(value).toLowerCase();
    if (!segment) return suggestions;
    return suggestions.filter((s) => s.toLowerCase().startsWith(segment));
  });

  // Re-fetch suggestions and reset when repo changes
  $effect(() => {
    if (repo) {
      suggestions = [];
      validationError = null;
      if (value.trim()) {
        fetchSuggestions(getParentPath(value));
        scheduleValidation();
      }
    }
  });
</script>

<div class="subpath-input-wrapper">
  <div class="input-container" class:has-error={validationError !== null}>
    <input
      bind:this={inputEl}
      class="subpath-input"
      type="text"
      bind:value
      oninput={handleInput}
      onfocus={handleFocus}
      onblur={handleBlur}
      onkeydown={handleKeydown}
      id="project-subpath"
      placeholder="e.g., packages/frontend"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      spellcheck={false}
      {disabled}
    />
    {#if validating}
      <div class="input-spinner">
        <Spinner size={14} />
      </div>
    {/if}
  </div>

  {#if showDropdown && filteredSuggestions.length > 0 && !disabled}
    <div class="suggestions-dropdown">
      {#each filteredSuggestions as dir, i}
        <button
          class="suggestion-item"
          class:highlighted={i === highlightedIndex}
          onmousedown={(e) => {
            e.preventDefault();
            selectSuggestion(dir);
          }}
        >
          <FolderOpen size={14} />
          <span class="suggestion-name">{dir}</span>
        </button>
      {/each}
    </div>
  {/if}

  {#if validationError}
    <div class="validation-error">{validationError}</div>
  {/if}
</div>

<style>
  .subpath-input-wrapper {
    position: relative;
  }

  .input-container {
    position: relative;
    display: flex;
    align-items: center;
  }

  .subpath-input {
    width: 100%;
    min-height: 42px;
    border: 1.5px solid var(--border-muted);
    background: transparent;
    color: var(--text-primary);
    border-radius: 10px;
    padding: 10px 36px 10px 14px;
    font-size: var(--size-md);
    font-family: inherit;
    outline: none;
    transition: border-color 0.15s ease;
    box-sizing: border-box;
  }

  .subpath-input:focus {
    border-color: var(--ui-accent);
  }

  .subpath-input::placeholder {
    color: var(--text-faint);
  }

  .subpath-input:disabled {
    opacity: 0.6;
  }

  .input-container.has-error .subpath-input {
    border-color: var(--ui-danger);
  }

  .input-spinner {
    position: absolute;
    right: 12px;
    top: 50%;
    transform: translateY(-50%);
    color: var(--text-faint);
    display: flex;
    align-items: center;
  }

  .suggestions-dropdown {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    margin-top: 4px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    max-height: 200px;
    overflow-y: auto;
    z-index: 100;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
  }

  .suggestion-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 12px;
    border: none;
    background: transparent;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    cursor: pointer;
    text-align: left;
    transition: background-color 0.1s ease;
  }

  .suggestion-item:hover,
  .suggestion-item.highlighted {
    background: var(--bg-hover);
  }

  .suggestion-item :global(svg) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .suggestion-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .validation-error {
    color: var(--ui-danger);
    font-size: var(--size-xs);
    margin-top: 4px;
  }
</style>
