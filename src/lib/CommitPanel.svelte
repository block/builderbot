<!--
  CommitPanel.svelte - Commit message input and actions
  
  Fixed footer panel for composing commit messages. Supports regular commits
  and amending the previous commit. Validates that staged changes exist
  before allowing commit.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { getGitStatus, getLastCommitMessage, createCommit, amendCommit } from './services/git';

  let commitMessage = $state('');
  let amendMode = $state(false);
  let currentBranch = $state<string | null>(null);
  let isCommitting = $state(false);
  let error = $state<string | null>(null);
  let stagedCount = $state(0);

  // Event to notify parent that a commit happened
  interface Props {
    onCommitComplete?: () => void;
  }
  let { onCommitComplete }: Props = $props();

  onMount(() => {
    loadStatus();
  });

  async function loadStatus() {
    try {
      const status = await getGitStatus();
      currentBranch = status.branch;
      stagedCount = status.staged.length;
    } catch (e) {
      console.error('Failed to get status:', e);
    }
  }

  // Expose refresh method for parent to call
  export function refresh() {
    loadStatus();
  }

  async function loadLastCommitMessage() {
    try {
      const message = await getLastCommitMessage();
      if (message) {
        commitMessage = message.trim();
      }
    } catch (e) {
      console.error('Failed to get last commit message:', e);
    }
  }

  function handleAmendToggle() {
    amendMode = !amendMode;
    if (amendMode) {
      loadLastCommitMessage();
    } else {
      commitMessage = '';
    }
  }

  async function handleCommit() {
    if (!commitMessage.trim() || isCommitting) return;

    isCommitting = true;
    error = null;

    try {
      if (amendMode) {
        await amendCommit(commitMessage);
      } else {
        await createCommit(commitMessage);
      }

      // Success - clear form and notify parent
      commitMessage = '';
      amendMode = false;
      onCommitComplete?.();
      await loadStatus();
    } catch (e) {
      error = e as string;
    } finally {
      isCommitting = false;
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    // Cmd/Ctrl + Enter to commit
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      handleCommit();
    }
  }
</script>

<div class="commit-panel-content">
  <div class="branch-info">
    <span class="branch-icon">⎇</span>
    <span class="branch-name">{currentBranch ?? 'loading...'}</span>
    {#if stagedCount > 0}
      <span class="staged-count">{stagedCount} staged</span>
    {/if}
  </div>

  <div class="commit-form">
    <input
      type="text"
      class="commit-input"
      placeholder={amendMode ? 'Amend commit message' : 'Commit message'}
      bind:value={commitMessage}
      onkeydown={handleKeydown}
      disabled={isCommitting}
    />

    <div class="commit-options">
      <label class="amend-checkbox">
        <input type="checkbox" checked={amendMode} onchange={handleAmendToggle} />
        <span>Amend</span>
      </label>
    </div>

    <div class="commit-actions">
      <button
        class="btn btn-primary"
        onclick={handleCommit}
        disabled={!commitMessage.trim() || isCommitting || (!amendMode && stagedCount === 0)}
      >
        {#if isCommitting}
          Committing...
        {:else if amendMode}
          Amend
        {:else}
          Commit
        {/if}
      </button>
    </div>
  </div>

  {#if error}
    <div class="error-message">{error}</div>
  {/if}
</div>

<style>
  .commit-panel-content {
    display: flex;
    flex-direction: column;
    height: 100%;
    padding: 12px 16px;
    box-sizing: border-box;
    gap: 8px;
  }

  .branch-info {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .branch-icon {
    font-size: var(--size-lg);
  }

  .branch-name {
    color: var(--text-link);
    font-weight: 500;
  }

  .staged-count {
    margin-left: auto;
    color: var(--status-added);
    font-size: var(--size-xs);
  }

  .commit-form {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: 1;
  }

  .commit-input {
    flex: 1;
    padding: 8px 12px;
    font-size: var(--size-md);
    background-color: var(--bg-input);
    border: 1px solid var(--border-primary);
    border-radius: 4px;
    color: var(--text-primary);
    outline: none;
  }

  .commit-input:focus {
    border-color: var(--ui-accent);
  }

  .commit-input::placeholder {
    color: var(--text-muted);
  }

  .commit-input:disabled {
    opacity: 0.7;
  }

  .commit-options {
    display: flex;
    align-items: center;
  }

  .amend-checkbox {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-sm);
    color: var(--text-muted);
    cursor: pointer;
  }

  .amend-checkbox input {
    cursor: pointer;
  }

  .commit-actions {
    display: flex;
    gap: 8px;
  }

  .btn {
    padding: 8px 16px;
    font-size: var(--size-md);
    border: none;
    border-radius: 4px;
    cursor: pointer;
    font-weight: 500;
    min-width: 80px;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-primary {
    background-color: var(--ui-accent);
    color: white;
  }

  .btn-primary:hover:not(:disabled) {
    background-color: var(--ui-accent-hover);
  }

  .error-message {
    color: var(--status-deleted);
    font-size: var(--size-sm);
    padding: 4px 0;
  }
</style>
