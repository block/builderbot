<!--
  NewSessionModal.svelte — Start a new commit or note session on a branch

  A focused modal with a prompt textarea. The mode (commit or note) is
  determined by the caller and displayed as a static title in the header.
  On close, returns whatever text was typed and the current mode so the
  caller can restore state if the user re-opens the modal.

  Props:
    branch        — the branch to create a session on
    mode          — 'commit' or 'note' (shown as title, not togglable)
    initialPrompt — pre-fill the textarea (e.g. from a previous close)
    onClose       — called with { prompt, mode } when dismissed
    onStarted     — called with { sessionId, artifactId } on successful start
-->
<script lang="ts">
  import { X, GitCommitVertical, FileText, FileSearch, GitBranch, Send } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import type { Branch, BranchSessionType } from '../../types';
  import * as commands from '../../commands';
  import AgentSelector from '../agents/AgentSelector.svelte';
  import { agentState, REMOTE_AGENTS } from '../agents/agent.svelte';
  import { getPreferredAgent } from '../settings/preferences.svelte';
  import { alerts } from '../../shared/alerts.svelte';

  interface Props {
    branch: Branch;
    mode: BranchSessionType;
    initialPrompt?: string;
    remote?: boolean;
    onClose: (draft: { prompt: string; mode: BranchSessionType }) => void;
    onStarted: (result: { sessionId: string; artifactId: string }) => void;
  }

  let { branch, mode, initialPrompt = '', remote = false, onClose, onStarted }: Props = $props();

  let prompt = $state('');
  let currentMode = $state<BranchSessionType>('commit');
  let starting = $state(false);
  let initialized = false;
  let textareaEl: HTMLTextAreaElement | null = $state(null);

  let isCommit = $derived(currentMode === 'commit');
  let isReview = $derived(currentMode === 'review');

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

  async function handleSubmit(e?: Event) {
    e?.preventDefault();
    // Review mode allows empty prompts; other modes require text
    if (!isReview && !prompt.trim()) return;
    if (starting) return;

    starting = true;

    try {
      const agents = remote ? REMOTE_AGENTS : agentState.providers;
      const finalPrompt =
        prompt.trim() || (isReview ? 'Review the code changes on this branch.' : '');
      const result = await commands.startBranchSession(
        branch.id,
        finalPrompt,
        currentMode,
        getPreferredAgent(agents) ?? undefined
      );
      onStarted({ sessionId: result.sessionId, artifactId: result.artifactId });
    } catch (e) {
      alerts.show({
        tone: 'error',
        title: 'Unable to start session',
        message: e instanceof Error ? e.message : String(e),
        durationMs: 0,
      });
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
    if (e.key === 'Enter' && e.metaKey && (prompt.trim() || isReview) && !starting) {
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
      <div class="header-title">
        {#if isReview}
          <span class="header-icon review-icon"><FileSearch size={14} /></span>
          <span>New AI review</span>
        {:else if isCommit}
          <span class="header-icon commit-icon"><GitCommitVertical size={14} /></span>
          <span>New commit</span>
        {:else}
          <span class="header-icon note-icon"><FileText size={14} /></span>
          <span>New note</span>
        {/if}
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
          placeholder={isReview
            ? 'Optional: focus the review on specific areas…'
            : isCommit
              ? 'Describe the change…'
              : 'Describe the note…'}
          rows={isReview ? 4 : 12}
          disabled={starting}
        ></textarea>
        <span class="hint">⌘ Enter to start</span>
      </div>

      <div class="form-actions">
        <AgentSelector disabled={starting} {remote} />
        <div class="form-actions-right">
          <button type="button" class="cancel-btn" onclick={handleClose} disabled={starting}>
            Cancel
          </button>
          <button
            type="submit"
            class="submit-btn"
            disabled={starting || (!isReview && !prompt.trim())}
          >
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

  .header-title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  .header-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .header-icon.note-icon {
    background-color: var(--note-bg);
    color: var(--note-color);
  }

  .header-icon.commit-icon {
    background-color: var(--commit-bg);
    color: var(--commit-color);
  }

  .header-icon.review-icon {
    background-color: var(--review-bg);
    color: var(--review-color);
  }

  .header-icon :global(svg) {
    flex-shrink: 0;
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
    color: var(--branch-color);
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
</style>
