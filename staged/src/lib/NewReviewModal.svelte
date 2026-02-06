<!--
  NewReviewModal.svelte - Modal for starting a code review on a branch

  Starts an AI-powered code review that analyzes the diff and provides feedback.
  The review is stored as a BranchNote with a special title prefix.
-->
<script lang="ts">
  import { X, FileSearch, Loader2 } from 'lucide-svelte';
  import type { Branch } from './services/branch';
  import * as branchService from './services/branch';
  import AgentSelector from './AgentSelector.svelte';
  import type { AcpProvider } from './stores/agent.svelte';
  import { preferences } from './stores/preferences.svelte';

  interface Props {
    branch: Branch;
    onClose: () => void;
    onReviewStarted: (branchNoteId: string, aiSessionId: string, provider: AcpProvider) => void;
  }

  let { branch, onClose, onReviewStarted }: Props = $props();

  let focusAreas = $state('');
  let submitting = $state(false);
  let error = $state<string | null>(null);
  let selectedProvider = $state<AcpProvider>((preferences.aiAgent as AcpProvider) || 'goose');

  async function handleSubmit(e: Event) {
    e.preventDefault();

    submitting = true;
    error = null;

    try {
      // Build a code review prompt
      const reviewPrompt = buildReviewPrompt(focusAreas.trim());

      // Use the existing note infrastructure with a special title
      const response = await branchService.startBranchNote(
        branch.id,
        'Code Review',
        reviewPrompt,
        selectedProvider
      );
      onReviewStarted(response.branchNoteId, response.aiSessionId, selectedProvider);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      submitting = false;
    }
  }

  function buildReviewPrompt(focusAreas: string): string {
    let prompt = `Please perform a thorough code review of the changes on this branch.

Review the diff between the base branch and the current HEAD. For each file with changes:

1. **Summary**: Briefly describe what the changes do
2. **Issues**: Identify any bugs, security concerns, or logic errors
3. **Suggestions**: Recommend improvements for code quality, readability, or performance
4. **Questions**: Note anything that's unclear or needs clarification

Format your review as markdown with clear sections for each file. Use code blocks to reference specific lines when discussing issues or suggestions.

At the end, provide an overall assessment:
- Is this ready to merge?
- What are the most important items to address?
- Any architectural concerns?`;

    if (focusAreas) {
      prompt += `\n\n**Specific areas to focus on:**\n${focusAreas}`;
    }

    return prompt;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      onClose();
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
        <FileSearch size={18} />
        <span class="header-title">Code Review</span>
      </div>
      <button class="close-btn" onclick={onClose}>
        <X size={18} />
      </button>
    </header>

    <form class="modal-content" onsubmit={handleSubmit}>
      <p class="description">
        Start an AI-powered code review of the changes on this branch. The AI will analyze the diff
        and provide feedback on code quality, potential issues, and suggestions for improvement.
      </p>

      <div class="form-group">
        <label for="focus-areas">Focus areas (optional)</label>
        <textarea
          id="focus-areas"
          bind:value={focusAreas}
          placeholder="e.g., Pay special attention to error handling, check for security issues in the auth code, review the database queries for performance..."
          rows={3}
          disabled={submitting}
        ></textarea>
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
          <button type="submit" class="submit-btn" disabled={submitting}>
            {#if submitting}
              <Loader2 size={14} class="spinning" />
              Starting...
            {:else}
              Start Review
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

  .description {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
    line-height: 1.5;
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

  .form-group textarea {
    padding: 10px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-md);
    font-family: inherit;
    transition: border-color 0.15s ease;
    resize: vertical;
    min-height: 60px;
  }

  .form-group textarea:focus {
    outline: none;
    border-color: var(--ui-accent);
  }

  .form-group textarea::placeholder {
    color: var(--text-faint);
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
    transform-origin: center;
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
