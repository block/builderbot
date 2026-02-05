<!--
  NewSessionModal.svelte - Start a new agent session on a branch

  Simple modal with a prompt input. The session will run in the branch's
  worktree and produce a commit when complete.

  The backend handles all context gathering (commits, notes, etc.) and
  builds the full prompt with timeline context.
-->
<script lang="ts">
  import { X, GitCommitHorizontal, GitBranch, Loader2, Send } from 'lucide-svelte';
  import type { Branch } from './services/branch';
  import { startBranchSession } from './services/branch';
  import AgentSelector from './AgentSelector.svelte';
  import type { AcpProvider } from './stores/agent.svelte';
  import { preferences } from './stores/preferences.svelte';

  interface Props {
    branch: Branch;
    onClose: () => void;
    onSessionStarted?: (branchSessionId: string, aiSessionId: string) => void;
  }

  let { branch, onClose, onSessionStarted }: Props = $props();

  // State
  let prompt = $state('');
  let starting = $state(false);
  let error = $state<string | null>(null);
  let selectedProvider = $state<AcpProvider>((preferences.aiAgent as AcpProvider) || 'goose');

  let textareaEl: HTMLTextAreaElement | null = $state(null);

  // Focus textarea on mount
  $effect(() => {
    if (textareaEl) {
      textareaEl.focus();
    }
  });

  // Extract repo name from path
  function repoName(path: string): string {
    const parts = path.split('/');
    return parts[parts.length - 1] || path;
  }

  async function handleStart(e: Event) {
    e.preventDefault();
    if (!prompt.trim()) return;

    starting = true;
    error = null;

    try {
      const userPrompt = prompt.trim();

      // Backend handles all context gathering and prompt building
      const result = await startBranchSession(branch.id, userPrompt, selectedProvider);

      // Notify parent that session started
      onSessionStarted?.(result.branchSessionId, result.aiSessionId);
      onClose();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      starting = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
      return;
    }

    // Cmd+Enter to submit
    if (e.key === 'Enter' && e.metaKey && prompt.trim() && !starting) {
      e.preventDefault();
      handleStart(e);
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
        <GitCommitHorizontal size={18} />
        <span class="header-title">New Commit</span>
      </div>
      <button class="close-btn" onclick={onClose}>
        <X size={18} />
      </button>
    </header>

    <form class="modal-content" onsubmit={handleStart}>
      <div class="branch-info">
        <GitBranch size={16} />
        <span class="branch-name">{branch.branchName}</span>
        <span class="repo-name">in {repoName(branch.repoPath)}</span>
      </div>

      <div class="form-group">
        <label for="prompt">What would you like to work on?</label>
        <textarea
          bind:this={textareaEl}
          bind:value={prompt}
          id="prompt"
          placeholder="Describe the task..."
          rows={4}
          disabled={starting}
        ></textarea>
        <p class="hint">Press ⌘Enter to start</p>
      </div>

      {#if error}
        <div class="error-message">{error}</div>
      {/if}

      <div class="form-actions">
        <AgentSelector bind:provider={selectedProvider} disabled={starting} />
        <div class="action-buttons">
          <button type="button" class="cancel-btn" onclick={onClose} disabled={starting}>
            Cancel
          </button>
          <button type="submit" class="submit-btn" disabled={starting || !prompt.trim()}>
            {#if starting}
              <Loader2 size={14} class="spinning" />
              Starting...
            {:else}
              <Send size={14} />
              Start Session
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

  .branch-info {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    background-color: var(--bg-hover);
    border-radius: 6px;
    font-size: var(--size-sm);
  }

  .branch-info :global(svg) {
    color: var(--status-renamed);
  }

  .branch-name {
    font-weight: 500;
    color: var(--text-primary);
  }

  .repo-name {
    color: var(--text-muted);
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
    resize: vertical;
    min-height: 80px;
    transition: border-color 0.15s ease;
  }

  .form-group textarea:focus {
    outline: none;
    border-color: var(--ui-accent);
  }

  .form-group textarea::placeholder {
    color: var(--text-faint);
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
