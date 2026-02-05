<!--
  NewNoteModal.svelte - Modal for creating a new note on a branch

  Prompts for a title and description, then starts AI generation.
  The backend handles all context gathering and prompt building.
-->
<script lang="ts">
  import { X, FileText, Loader2 } from 'lucide-svelte';
  import type { Branch } from './services/branch';
  import * as branchService from './services/branch';
  import AgentSelector from './AgentSelector.svelte';
  import type { AcpProvider } from './stores/agent.svelte';
  import { preferences } from './stores/preferences.svelte';

  interface Props {
    branch: Branch;
    onClose: () => void;
    onNoteStarted: (branchNoteId: string, aiSessionId: string, provider: AcpProvider) => void;
  }

  let { branch, onClose, onNoteStarted }: Props = $props();

  let title = $state('');
  let description = $state('');
  let submitting = $state(false);
  let error = $state<string | null>(null);
  let selectedProvider = $state<AcpProvider>((preferences.aiAgent as AcpProvider) || 'goose');

  async function handleSubmit(e: Event) {
    e.preventDefault();

    if (!title.trim()) {
      error = 'Please enter a title';
      return;
    }

    submitting = true;
    error = null;

    try {
      // Backend handles all context gathering and prompt building
      const response = await branchService.startBranchNote(
        branch.id,
        title.trim(),
        description.trim(),
        selectedProvider
      );
      onNoteStarted(response.branchNoteId, response.aiSessionId, selectedProvider);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      submitting = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
      return;
    }

    // Cmd+Enter to submit
    if (e.key === 'Enter' && e.metaKey && title.trim() && !submitting) {
      e.preventDefault();
      handleSubmit(e);
      return;
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="modal-backdrop" role="presentation" onclick={onClose}>
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div
    class="modal"
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
  >
    <header class="modal-header">
      <div class="header-content">
        <FileText size={18} />
        <span class="header-title">New Note</span>
      </div>
      <button class="close-btn" onclick={onClose}>
        <X size={18} />
      </button>
    </header>

    <form class="modal-content" onsubmit={handleSubmit}>
      <div class="form-group">
        <label for="title">Title</label>
        <input
          id="title"
          type="text"
          bind:value={title}
          placeholder="e.g., Architecture Overview"
          disabled={submitting}
        />
      </div>

      <div class="form-group">
        <label for="description">Description</label>
        <textarea
          id="description"
          bind:value={description}
          placeholder="What should this note cover? The AI will generate the content based on your description and the current codebase."
          rows={4}
          disabled={submitting}
        ></textarea>
        <p class="hint">Press ⌘Enter to generate</p>
      </div>

      {#if error}
        <div class="error-message">{error}</div>
      {/if}

      <div class="form-actions">
        <AgentSelector bind:provider={selectedProvider} disabled={submitting} />
        <div class="action-buttons">
          <button type="button" class="cancel-btn" onclick={onClose} disabled={submitting}>
            Cancel
          </button>
          <button type="submit" class="submit-btn" disabled={submitting || !title.trim()}>
            {#if submitting}
              <Loader2 size={14} class="spinning" />
              Generating...
            {:else}
              Generate Note
            {/if}
          </button>
        </div>
      </div>
    </form>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    display: flex;
    flex-direction: column;
    width: 90%;
    max-width: 500px;
    background: var(--bg-chrome);
    border-radius: 12px;
    overflow: hidden;
    box-shadow: 0 20px 40px rgba(0, 0, 0, 0.3);
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .header-content {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-primary);
  }

  .header-content :global(svg) {
    color: var(--text-accent);
  }

  .header-title {
    font-size: var(--size-md);
    font-weight: 500;
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 6px;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .close-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .modal-content {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .form-group label {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-muted);
  }

  .form-group input,
  .form-group textarea {
    padding: 10px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-md);
    font-family: inherit;
    transition: border-color 0.15s ease;
  }

  .form-group input:focus,
  .form-group textarea:focus {
    outline: none;
    border-color: var(--ui-accent);
  }

  .form-group input::placeholder,
  .form-group textarea::placeholder {
    color: var(--text-faint);
  }

  .form-group textarea {
    resize: vertical;
    min-height: 80px;
  }

  .hint {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-faint);
    text-align: right;
  }

  .error-message {
    padding: 10px 12px;
    background: var(--ui-danger-bg);
    border-radius: 6px;
    color: var(--ui-danger);
    font-size: var(--size-sm);
  }

  .form-actions {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
    margin-top: 8px;
  }

  .action-buttons {
    display: flex;
    gap: 10px;
  }

  .cancel-btn,
  .submit-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 10px 16px;
    border-radius: 6px;
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .cancel-btn {
    background: transparent;
    border: 1px solid var(--border-muted);
    color: var(--text-muted);
  }

  .cancel-btn:hover:not(:disabled) {
    border-color: var(--text-primary);
    color: var(--text-primary);
  }

  .submit-btn {
    background: var(--ui-accent);
    border: none;
    color: var(--bg-deepest);
  }

  .submit-btn:hover:not(:disabled) {
    background: var(--ui-accent-hover);
  }

  .submit-btn:disabled,
  .cancel-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  :global(.spinning) {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
