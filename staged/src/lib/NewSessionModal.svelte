<!--
  NewSessionModal.svelte — Start a new commit or note session on a branch

  A focused modal with a prompt textarea and mode toggle (commit/note).
  On close, returns whatever text was typed and the current mode so the
  caller can restore state if the user re-opens the modal.

  Props:
    branch        — the branch to create a session on
    mode          — initial mode: 'commit' or 'note'
    initialPrompt — pre-fill the textarea (e.g. from a previous close)
    onClose       — called with { prompt, mode } when dismissed
    onStarted     — called with { sessionId, artifactId } on successful start
-->
<script lang="ts">
  import { X, GitCommitHorizontal, StickyNote, GitBranch, Send } from 'lucide-svelte';
  import Spinner from './Spinner.svelte';
  import type { Branch, BranchSessionType } from './types';
  import * as commands from './commands';
  import AgentSelector from './AgentSelector.svelte';
  import { preferences } from './stores/preferences.svelte';

  interface Props {
    branch: Branch;
    mode: BranchSessionType;
    initialPrompt?: string;
    onClose: (draft: { prompt: string; mode: BranchSessionType }) => void;
    onStarted: (result: { sessionId: string; artifactId: string }) => void;
  }

  let { branch, mode, initialPrompt = '', onClose, onStarted }: Props = $props();

  let prompt = $state('');
  let currentMode = $state<BranchSessionType>('commit');
  let starting = $state(false);
  let initialized = false;
  let error = $state<string | null>(null);
  let textareaEl: HTMLTextAreaElement | null = $state(null);

  let isCommit = $derived(currentMode === 'commit');

  // Seed prompt and mode from props once; caller preserves draft across open/close.
  $effect(() => {
    if (!initialized) {
      initialized = true;
      prompt = initialPrompt;
      currentMode = mode;
    }
  });

  // Focus textarea on mount (one-time)
  $effect(() => {
    if (textareaEl) {
      const el = textareaEl;
      // Read length from the DOM element to avoid tracking `prompt` reactively,
      // which would re-run this effect on every keystroke and force the cursor
      // to the end of the buffer.
      el.focus();
      el.selectionStart = el.selectionEnd = el.value.length;
    }
  });

  function toggleMode() {
    currentMode = currentMode === 'commit' ? 'note' : 'commit';
  }

  async function handleSubmit(e?: Event) {
    e?.preventDefault();
    if (!prompt.trim() || starting) return;

    starting = true;
    error = null;

    try {
      const result = await commands.startBranchSession(
        branch.id,
        prompt.trim(),
        currentMode,
        preferences.aiAgent ?? undefined
      );
      onStarted({ sessionId: result.sessionId, artifactId: result.artifactId });
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      starting = false;
    }
  }

  function handleClose() {
    onClose({ prompt, mode: currentMode });
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      handleClose();
      return;
    }

    // Cmd+Enter to submit
    if (e.key === 'Enter' && e.metaKey && prompt.trim() && !starting) {
      e.preventDefault();
      handleSubmit();
    }
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      handleClose();
    }
  }

  function formatBaseBranch(baseBranch: string): string {
    return baseBranch.replace(/^origin\//, '');
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={(e) => e.key === 'Escape' && handleClose()}
>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal" role="presentation" onclick={(e) => e.stopPropagation()}>
    <header class="modal-header">
      <div class="header-content">
        <!-- Mode toggle -->
        <button
          type="button"
          class="mode-toggle"
          onclick={toggleMode}
          disabled={starting}
          title="Switch to {isCommit ? 'note' : 'commit'}"
        >
          <span class="mode-option" class:active={isCommit}>
            <GitCommitHorizontal size={14} />
            <span>Commit</span>
          </span>
          <span class="mode-option" class:active={!isCommit}>
            <StickyNote size={14} />
            <span>Note</span>
          </span>
        </button>
      </div>
      <button class="close-btn" onclick={handleClose} title="Close (Esc)">
        <X size={18} />
      </button>
    </header>

    <form class="modal-body" onsubmit={handleSubmit}>
      <div class="branch-info">
        <GitBranch size={14} />
        <span class="branch-name">{branch.branchName}</span>
        <span class="branch-sep">›</span>
        <span class="base-name">{formatBaseBranch(branch.baseBranch)}</span>
      </div>

      <div class="form-group">
        <textarea
          bind:this={textareaEl}
          bind:value={prompt}
          placeholder={isCommit ? 'Describe the change…' : 'Describe the note…'}
          rows={12}
          disabled={starting}
        ></textarea>
        <span class="hint">⌘ Enter to start</span>
      </div>

      {#if error}
        <div class="error-message">{error}</div>
      {/if}

      <div class="form-actions">
        <AgentSelector disabled={starting} />
        <div class="form-actions-right">
          <button type="button" class="cancel-btn" onclick={handleClose} disabled={starting}>
            Cancel
          </button>
          <button type="submit" class="submit-btn" disabled={starting || !prompt.trim()}>
            {#if starting}
              <Spinner size={14} />
              Starting…
            {:else}
              <Send size={14} />
              Start
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
    background: var(--shadow-overlay);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 12vh;
    z-index: 1000;
  }

  .modal {
    display: flex;
    flex-direction: column;
    width: 580px;
    max-width: 90vw;
    background: var(--bg-chrome);
    border-radius: 12px;
    overflow: hidden;
    box-shadow: var(--shadow-elevated);
  }

  /* Header */
  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 18px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .header-content {
    display: flex;
    align-items: center;
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

  /* Mode toggle — pill-shaped segmented control */
  .mode-toggle {
    display: flex;
    align-items: center;
    gap: 0;
    padding: 2px;
    background: var(--bg-hover);
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    cursor: pointer;
    transition: border-color 0.15s;
  }

  .mode-toggle:hover:not(:disabled) {
    border-color: var(--border-muted);
  }

  .mode-toggle:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .mode-option {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 5px 10px;
    border-radius: 6px;
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-faint);
    transition:
      color 0.15s,
      background-color 0.15s;
  }

  .mode-option :global(svg) {
    flex-shrink: 0;
  }

  .mode-option.active {
    color: var(--text-primary);
    background: var(--bg-primary);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.08);
  }

  /* Body */
  .modal-body {
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .branch-info {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    background: var(--bg-hover);
    border-radius: 6px;
    font-size: var(--size-sm);
  }

  .branch-info :global(svg) {
    color: var(--status-renamed);
    flex-shrink: 0;
  }

  .branch-name {
    font-weight: 500;
    color: var(--text-primary);
  }

  .branch-sep {
    color: var(--text-faint);
  }

  .base-name {
    color: var(--text-muted);
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .form-group textarea {
    padding: 10px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    line-height: 1.5;
    resize: vertical;
    min-height: 240px;
    transition: border-color 0.15s;
  }

  .form-group textarea:focus {
    outline: none;
    border-color: var(--border-emphasis);
  }

  .form-group textarea::placeholder {
    color: var(--text-faint);
  }

  .form-group textarea:disabled {
    opacity: 0.6;
  }

  .hint {
    font-size: var(--size-xs);
    color: var(--text-faint);
    text-align: right;
  }

  .error-message {
    padding: 8px 12px;
    background: var(--ui-danger-bg);
    border-radius: 6px;
    color: var(--ui-danger);
    font-size: var(--size-sm);
  }

  /* Actions */
  .form-actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-top: 4px;
  }

  .form-actions-right {
    display: flex;
    gap: 8px;
  }

  .cancel-btn,
  .submit-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border-radius: 6px;
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
  }

  .cancel-btn {
    background: transparent;
    border: 1px solid var(--border-muted);
    color: var(--text-muted);
  }

  .cancel-btn:hover:not(:disabled) {
    border-color: var(--border-emphasis);
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
</style>
