<!--
  NewSessionModal.svelte - Start a new agent session on a branch

  Simple modal with a prompt input. The session will run in the branch's
  worktree and produce a commit when complete.
-->
<script lang="ts">
  import { X, GitBranch, Loader2, Send } from 'lucide-svelte';
  import type { Branch, CommitInfo, BranchSession, BranchNote } from './services/branch';
  import { startBranchSession } from './services/branch';
  import { buildTimelineContext } from './services/timelineContext';

  interface Props {
    branch: Branch;
    commits?: CommitInfo[];
    sessionsByCommit?: Map<string, BranchSession>;
    notes?: BranchNote[];
    onClose: () => void;
    onSessionStarted?: (branchSessionId: string, aiSessionId: string) => void;
  }

  let {
    branch,
    commits = [],
    sessionsByCommit = new Map(),
    notes = [],
    onClose,
    onSessionStarted,
  }: Props = $props();

  // State
  let prompt = $state('');
  let starting = $state(false);
  let error = $state<string | null>(null);

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

  // Build the full prompt with instructions for commit-focused work
  function buildCommitPrompt(userPrompt: string): string {
    const context = buildTimelineContext({
      branchName: branch.branchName,
      baseBranch: branch.baseBranch,
      commits,
      sessionsByCommit,
      notes,
    });

    const contextBlock = context ? `${context}\n\n` : '';

    return `${contextBlock}You are working on a feature branch. Your goal is to complete the following task and create a git commit with your changes.

TASK: ${userPrompt}

Guidelines:
- Make the necessary code changes to complete the task
- When finished, create a git commit with a clear, descriptive commit message
- The commit message should summarize what was done
- Keep changes focused and atomic - one logical change per session

Begin working on the task now.`;
  }

  async function handleStart() {
    if (!prompt.trim()) return;

    starting = true;
    error = null;

    try {
      const fullPrompt = buildCommitPrompt(prompt.trim());
      const result = await startBranchSession(branch.id, fullPrompt);

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
    if (e.key === 'Enter' && e.metaKey && prompt.trim()) {
      e.preventDefault();
      handleStart();
      return;
    }
  }
</script>

<div class="modal-backdrop" role="button" tabindex="-1" onclick={onClose} onkeydown={handleKeydown}>
  <div
    class="modal"
    role="dialog"
    tabindex="-1"
    onkeydown={() => {}}
    onclick={(e) => e.stopPropagation()}
  >
    <div class="modal-header">
      <h2>New Session</h2>
      <button class="close-button" onclick={onClose}>
        <X size={18} />
      </button>
    </div>

    <div class="modal-content">
      <div class="branch-info">
        <GitBranch size={16} />
        <span class="branch-name">{branch.branchName}</span>
        <span class="repo-name">in {repoName(branch.repoPath)}</span>
      </div>

      <div class="prompt-section">
        <label for="prompt">What would you like to work on?</label>
        <textarea
          bind:this={textareaEl}
          bind:value={prompt}
          id="prompt"
          placeholder="Describe the task..."
          rows="4"
        ></textarea>
        <p class="hint">Press ⌘Enter to start</p>
      </div>

      {#if error}
        <p class="error">{error}</p>
      {/if}
    </div>

    <div class="modal-footer">
      <button class="cancel-button" onclick={onClose}>Cancel</button>
      <button class="start-button" onclick={handleStart} disabled={!prompt.trim() || starting}>
        {#if starting}
          <Loader2 size={14} class="spinner" />
        {:else}
          <Send size={14} />
        {/if}
        Start Session
      </button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background-color: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 15vh;
    z-index: 1000;
  }

  .modal {
    width: 500px;
    max-width: 90vw;
    background-color: var(--bg-primary);
    border-radius: 12px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.3);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .modal-header h2 {
    margin: 0;
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-primary);
  }

  .close-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
  }

  .close-button:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .modal-content {
    padding: 16px;
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

  .prompt-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .prompt-section label {
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .prompt-section textarea {
    padding: 12px;
    background-color: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    font-size: var(--size-md);
    font-family: inherit;
    color: var(--text-primary);
    resize: vertical;
    outline: none;
    transition: border-color 0.15s;
  }

  .prompt-section textarea:focus {
    border-color: var(--ui-accent);
  }

  .prompt-section textarea::placeholder {
    color: var(--text-faint);
  }

  .hint {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-faint);
    text-align: right;
  }

  .error {
    margin: 0;
    padding: 8px 12px;
    background-color: var(--ui-danger-bg);
    border-radius: 6px;
    font-size: var(--size-sm);
    color: var(--ui-danger);
  }

  .modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 16px;
    border-top: 1px solid var(--border-subtle);
  }

  .cancel-button {
    padding: 8px 16px;
    background: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: all 0.15s;
  }

  .cancel-button:hover {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
  }

  .start-button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    background-color: var(--ui-accent);
    border: none;
    border-radius: 6px;
    color: var(--bg-deepest);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.15s;
  }

  .start-button:hover:not(:disabled) {
    background-color: var(--ui-accent-hover);
  }

  .start-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* Spinner */
  :global(.spinner) {
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
