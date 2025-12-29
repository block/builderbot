<!--
  DiffSelectorModal.svelte - Custom ref selection for diff viewing
  
  Allows users to specify arbitrary base and head refs for viewing diffs.
  Provides autocomplete for branches and tags.
  Validates refs before applying and shows helpful error messages.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X } from 'lucide-svelte';
  import { getRefs, resolveRef } from './services/git';
  import type { GitRef } from './types';

  interface Props {
    open: boolean;
    onClose: () => void;
    onSelect: (base: string, head: string, label: string) => void;
    currentBase?: string;
    currentHead?: string;
  }

  let { open, onClose, onSelect, currentBase = 'HEAD', currentHead = '@' }: Props = $props();

  let baseRef = $state(currentBase);
  let headRef = $state(currentHead);
  let error = $state<string | null>(null);
  let validating = $state(false);

  // Autocomplete state
  let allRefs = $state<GitRef[]>([]);
  let activeInput = $state<'base' | 'head' | null>(null);
  let filteredRefs = $derived(
    activeInput
      ? allRefs.filter((r) => {
          const query = (activeInput === 'base' ? baseRef : headRef).toLowerCase();
          return r.name.toLowerCase().includes(query);
        })
      : []
  );
  let selectedSuggestionIndex = $state(0);

  // Load refs on mount
  onMount(async () => {
    try {
      allRefs = await getRefs();
    } catch (e) {
      console.error('Failed to load refs:', e);
    }
  });

  // Reset state when modal opens
  $effect(() => {
    if (open) {
      baseRef = currentBase;
      headRef = currentHead;
      error = null;
      validating = false;
      activeInput = null;
      selectedSuggestionIndex = 0;
    }
  });

  async function validateAndResolve(ref: string): Promise<string | null> {
    try {
      return await resolveRef(ref);
    } catch {
      return null;
    }
  }

  async function handleSubmit() {
    error = null;
    validating = true;
    activeInput = null;

    try {
      // Validate base ref
      const baseSha = await validateAndResolve(baseRef);
      if (!baseSha) {
        error = `Invalid base ref: "${baseRef}"`;
        validating = false;
        return;
      }

      // Validate head ref
      const headSha = await validateAndResolve(headRef);
      if (!headSha) {
        error = `Invalid head ref: "${headRef}"`;
        validating = false;
        return;
      }

      // Generate a label for the custom diff
      const label = `${baseRef}..${headRef}`;
      onSelect(baseRef, headRef, label);
      onClose();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      validating = false;
    }
  }

  function handleInputKeydown(event: KeyboardEvent, field: 'base' | 'head') {
    if (event.key === 'Escape') {
      if (activeInput) {
        activeInput = null;
        event.stopPropagation();
      } else {
        onClose();
      }
    } else if (event.key === 'Enter') {
      if (activeInput && filteredRefs.length > 0) {
        selectSuggestion(filteredRefs[selectedSuggestionIndex]);
        event.preventDefault();
      } else if (!validating) {
        handleSubmit();
      }
    } else if (event.key === 'ArrowDown' && activeInput) {
      event.preventDefault();
      selectedSuggestionIndex = Math.min(selectedSuggestionIndex + 1, filteredRefs.length - 1);
    } else if (event.key === 'ArrowUp' && activeInput) {
      event.preventDefault();
      selectedSuggestionIndex = Math.max(selectedSuggestionIndex - 1, 0);
    } else if (event.key === 'Tab' && activeInput && filteredRefs.length > 0) {
      event.preventDefault();
      selectSuggestion(filteredRefs[selectedSuggestionIndex]);
    }
  }

  function selectSuggestion(ref: GitRef) {
    if (activeInput === 'base') {
      baseRef = ref.name;
    } else if (activeInput === 'head') {
      headRef = ref.name;
    }
    activeInput = null;
    selectedSuggestionIndex = 0;
  }

  function handleFocus(field: 'base' | 'head') {
    activeInput = field;
    selectedSuggestionIndex = 0;
  }

  function handleBlur() {
    // Delay to allow click on suggestion
    setTimeout(() => {
      activeInput = null;
    }, 150);
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      onClose();
    }
  }

  function handleModalKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && !activeInput) {
      onClose();
    }
  }

  function getRefIcon(refType: string): string {
    switch (refType) {
      case 'branch':
        return '⎇';
      case 'tag':
        return '🏷';
      case 'special':
        return '◆';
      default:
        return '';
    }
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="modal-backdrop"
    onclick={handleBackdropClick}
    onkeydown={handleModalKeydown}
    role="dialog"
  >
    <div class="modal">
      <div class="modal-header">
        <h2>Custom Diff</h2>
        <button class="close-btn" onclick={onClose} title="Close">
          <X size={18} />
        </button>
      </div>

      <div class="modal-body">
        <div class="field">
          <label for="base-ref">Base (from)</label>
          <div class="input-wrapper">
            <input
              id="base-ref"
              type="text"
              bind:value={baseRef}
              placeholder="main, HEAD~3, v1.0, abc123..."
              disabled={validating}
              onfocus={() => handleFocus('base')}
              onblur={handleBlur}
              onkeydown={(e) => handleInputKeydown(e, 'base')}
              autocomplete="off"
            />
            {#if activeInput === 'base' && filteredRefs.length > 0}
              <div class="suggestions">
                {#each filteredRefs.slice(0, 8) as ref, i}
                  <button
                    class="suggestion"
                    class:selected={i === selectedSuggestionIndex}
                    onmousedown={() => selectSuggestion(ref)}
                  >
                    <span class="ref-icon">{getRefIcon(ref.ref_type)}</span>
                    <span class="ref-name">{ref.name}</span>
                    <span class="ref-type">{ref.ref_type}</span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        </div>

        <div class="arrow">↓</div>

        <div class="field">
          <label for="head-ref">Head (to)</label>
          <div class="input-wrapper">
            <input
              id="head-ref"
              type="text"
              bind:value={headRef}
              placeholder="@, HEAD, feature-branch..."
              disabled={validating}
              onfocus={() => handleFocus('head')}
              onblur={handleBlur}
              onkeydown={(e) => handleInputKeydown(e, 'head')}
              autocomplete="off"
            />
            {#if activeInput === 'head' && filteredRefs.length > 0}
              <div class="suggestions">
                {#each filteredRefs.slice(0, 8) as ref, i}
                  <button
                    class="suggestion"
                    class:selected={i === selectedSuggestionIndex}
                    onmousedown={() => selectSuggestion(ref)}
                  >
                    <span class="ref-icon">{getRefIcon(ref.ref_type)}</span>
                    <span class="ref-name">{ref.name}</span>
                    <span class="ref-type">{ref.ref_type}</span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        </div>

        {#if error}
          <div class="error">{error}</div>
        {/if}
      </div>

      <div class="modal-footer">
        <button class="btn btn-secondary" onclick={onClose} disabled={validating}>Cancel</button>
        <button class="btn btn-primary" onclick={handleSubmit} disabled={validating}>
          {#if validating}
            Validating...
          {:else}
            View Diff
          {/if}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    background: var(--bg-secondary);
    border: 1px solid var(--border-primary);
    border-radius: 8px;
    width: 360px;
    max-width: 90vw;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-primary);
  }

  .modal-header h2 {
    margin: 0;
    font-size: var(--size-md);
    font-weight: 600;
    color: var(--text-primary);
  }

  .close-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .close-btn:hover {
    background: var(--bg-tertiary);
    color: var(--text-primary);
  }

  .modal-body {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .field label {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-secondary);
  }

  .input-wrapper {
    position: relative;
  }

  .field input {
    width: 100%;
    padding: 8px 10px;
    background: var(--bg-input);
    border: 1px solid var(--border-primary);
    border-radius: 4px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: monospace;
    box-sizing: border-box;
  }

  .field input:focus {
    outline: none;
    border-color: var(--text-link);
  }

  .field input:disabled {
    opacity: 0.6;
  }

  .suggestions {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    margin-top: 4px;
    background: var(--bg-secondary);
    border: 1px solid var(--border-primary);
    border-radius: 4px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
    max-height: 200px;
    overflow-y: auto;
    z-index: 10;
  }

  .suggestion {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 10px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    text-align: left;
    cursor: pointer;
  }

  .suggestion:hover,
  .suggestion.selected {
    background: var(--bg-tertiary);
  }

  .ref-icon {
    font-size: var(--size-xs);
    width: 16px;
    text-align: center;
  }

  .ref-name {
    flex: 1;
    font-family: monospace;
  }

  .ref-type {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .arrow {
    text-align: center;
    color: var(--text-muted);
    font-size: var(--size-lg);
  }

  .error {
    padding: 8px 10px;
    background: rgba(255, 85, 85, 0.1);
    border: 1px solid var(--status-deleted);
    border-radius: 4px;
    color: var(--status-deleted);
    font-size: var(--size-sm);
  }

  .modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--border-primary);
  }

  .btn {
    padding: 6px 14px;
    border-radius: 4px;
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition:
      background-color 0.15s,
      border-color 0.15s;
  }

  .btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-secondary {
    background: var(--bg-tertiary);
    border: 1px solid var(--border-primary);
    color: var(--text-primary);
  }

  .btn-secondary:hover:not(:disabled) {
    background: var(--bg-input);
  }

  .btn-primary {
    background: var(--text-link);
    border: 1px solid var(--text-link);
    color: var(--bg-primary);
  }

  .btn-primary:hover:not(:disabled) {
    opacity: 0.9;
  }
</style>
