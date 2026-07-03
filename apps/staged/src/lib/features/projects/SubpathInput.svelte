<!--
  SubpathInput.svelte - Subpath input with directory autocomplete

  Features:
  - Directory suggestions dropdown from listRepoDirectories
  - Exposes waitForValidation() so parent can validate on submit
-->
<script lang="ts" module>
  export interface SubpathValidationResult {
    valid: boolean;
    error?: string;
  }

  export interface SubpathInputApi {
    waitForValidation(): Promise<SubpathValidationResult>;
  }
</script>

<script lang="ts">
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import * as commands from '../../api/commands';
  import { Input } from '$lib/components/ui/input';
  import {
    getSubpathCurrentSegment,
    getSubpathParentPath,
    isSubpathSuggestionVisible,
    normalizeSubpathInput,
  } from './subpathSuggestions';

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

  let suggestions = $state<string[]>([]);
  let showDropdown = $state(false);
  let highlightedIndex = $state(-1);
  let inputEl: HTMLInputElement | null = $state(null);

  // Expose the API to the parent via bindable prop
  $effect(() => {
    api = {
      waitForValidation,
    };
  });

  function validationErrorMessage(error: unknown): string {
    const message =
      typeof error === 'string' ? error : error instanceof Error ? error.message : String(error);
    return message.replace(/^git command failed:\s*/i, '') || 'Invalid path in repo';
  }

  /**
   * Fetch suggestions for the current input value.
   *
   * We always list directories at the parent path level so that the typed
   * segment can be matched against sibling directory names.  When one of
   * those siblings exactly matches the current segment we *also* fetch its
   * children so the dropdown can show both the directory itself and its
   * immediate subdirectories (e.g. typing "apps" shows "apps", "apps/one",
   * "apps/two").
   *
   * All entries in `suggestions` are stored as full paths from the repo root
   * so they can be displayed and selected without ambiguity.
   */
  async function fetchSuggestions(val: string) {
    if (!repo) {
      suggestions = [];
      return;
    }

    const parentPath = getSubpathParentPath(val);
    const segment = getSubpathCurrentSegment(val).toLowerCase();

    try {
      // 1. List directories at the parent level
      const siblingNames = await commands.listRepoDirectories(repo, parentPath);

      // Build full paths for siblings
      const siblingPaths = siblingNames.map((name) =>
        parentPath ? `${parentPath}/${name}` : name
      );

      // 2. If the typed segment exactly matches a sibling, also fetch its children
      const exactMatch = siblingNames.find((name) => name.toLowerCase() === segment);
      let childPaths: string[] = [];

      if (exactMatch) {
        const exactFullPath = parentPath ? `${parentPath}/${exactMatch}` : exactMatch;
        try {
          const childNames = await commands.listRepoDirectories(repo, exactFullPath);
          childPaths = childNames.map((name) => `${exactFullPath}/${name}`);
        } catch {
          // If fetching children fails, just show siblings
        }
      }

      // Combine: sibling paths + child paths (no duplicates)
      const combined = [...siblingPaths, ...childPaths];
      suggestions = [...new Set(combined)];
    } catch {
      suggestions = [];
    }
  }

  /**
   * Returns a promise that resolves to a validation result for the current
   * subpath. Called by the parent on submit.
   */
  async function waitForValidation(): Promise<SubpathValidationResult> {
    const trimmed = normalizeSubpathInput(value);
    if (!trimmed) {
      return { valid: true };
    }

    try {
      await commands.validateSubpath(repo, trimmed);
      return { valid: true };
    } catch (error) {
      return { valid: false, error: validationErrorMessage(error) };
    }
  }

  function handleInput() {
    highlightedIndex = -1;
    showDropdown = true;
    fetchSuggestions(value);
  }

  function handleFocus() {
    showDropdown = true;
    fetchSuggestions(value);
  }

  function handleBlur() {
    // Delay to allow click on dropdown items
    setTimeout(() => {
      showDropdown = false;
    }, 150);
  }

  /** Select a suggestion — `dir` is already a full path from the repo root. */
  function selectSuggestion(dir: string) {
    value = dir;
    showDropdown = false;
    highlightedIndex = -1;
    // Fetch next level of suggestions for the newly selected path
    fetchSuggestions(dir);
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

  // Filter suggestions: match full paths that start with the normalized input
  // and hide dot-prefixed directory segments until the user types "." in
  // that path segment.
  let filteredSuggestions = $derived.by(() => {
    return suggestions.filter((s) => isSubpathSuggestionVisible(s, value));
  });

  // Re-fetch suggestions when repo changes
  $effect(() => {
    if (repo) {
      suggestions = [];
      if (value.trim()) {
        fetchSuggestions(value);
      }
    }
  });
</script>

<div class="subpath-input-wrapper">
  <div class="input-container">
    <Input
      bind:ref={inputEl}
      class="min-h-[42px] rounded-[10px] bg-background px-3.5 py-2.5 text-base"
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
</style>
