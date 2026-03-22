<!--
  NewSessionModal.svelte — Start a new branch session on a branch

  A focused modal with a prompt textarea. The mode (commit/note/review) is
  determined by the caller and displayed as a static title in the header.
  On close, returns whatever text was typed and the current mode so the
  caller can restore state if the user re-opens the modal.

  Props:
    branch        — the branch to create a session on
    mode          — 'commit', 'note', or 'review' (shown as title, not togglable)
    initialPrompt — pre-fill the textarea (e.g. from a previous close)
    onClose       — called with { prompt, mode, imageIds } when dismissed
    onSubmit      — called with { prompt, mode, imageIds } when submit is pressed
-->
<script lang="ts">
  import { X, GitCommitVertical, FileText, FileSearch, GitBranch, Send } from 'lucide-svelte';
  import { tick, untrack } from 'svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import type { Branch, BranchSessionType } from '../../types';
  import AgentSelector from '../agents/AgentSelector.svelte';
  import ImageAttachment from './ImageAttachment.svelte';
  import { createBackdropDismissHandlers } from '../../shared/backdropDismiss';
  import { subscribeDragDrop } from '../branches/dragDrop';
  import { isImageFile } from '../branches/branchCardHelpers';
  import { createImage } from '../../commands';

  interface Props {
    branch: Branch;
    mode: BranchSessionType;
    initialPrompt?: string;
    initialImageIds?: string[];
    /** When true, the initial prompt is a suggestion — select all so typing replaces it. */
    prefilled?: boolean;
    remote?: boolean;
    /** When true, the session will be queued rather than started immediately. */
    willQueue?: boolean;
    onClose: (draft: { prompt: string; mode: BranchSessionType; imageIds: string[] }) => void;
    onSubmit: (data: { prompt: string; mode: BranchSessionType; imageIds: string[] }) => void;
  }

  let {
    branch,
    mode,
    initialPrompt = '',
    initialImageIds = [],
    prefilled = false,
    remote = false,
    willQueue = false,
    onClose,
    onSubmit,
  }: Props = $props();

  let prompt = $state('');
  let currentMode = $state<BranchSessionType>('commit');
  let starting = $state(false);
  let initialized = false;
  let textareaEl: HTMLTextAreaElement | null = $state(null);
  const backdropDismiss = createBackdropDismissHandlers({ onDismiss: handleClose });

  let isCommit = $derived(currentMode === 'commit');
  let isReview = $derived(currentMode === 'review');

  // Image attachment state
  let imageIds = $state<string[]>([]);

  // Drag-and-drop state
  let dragOver = $state(false);
  let modalElement: HTMLDivElement | undefined = $state();

  // Seed prompt and mode from props once; caller preserves draft across open/close.
  $effect(() => {
    if (!initialized) {
      initialized = true;
      prompt = initialPrompt;
      currentMode = mode;
      imageIds = [...initialImageIds];
    }
  });

  // Focus textarea on mount (one-time).
  // We await tick() so the DOM reflects the prompt value set by the init effect above.
  $effect(() => {
    if (textareaEl) {
      const el = textareaEl;
      const shouldSelect = prefilled;
      tick().then(() => {
        el.focus();
        if (shouldSelect && el.value.length > 0) {
          // Select all so typing replaces the suggested text
          el.selectionStart = 0;
          el.selectionEnd = el.value.length;
        } else {
          // Place cursor at end
          el.selectionStart = el.selectionEnd = el.value.length;
        }
      });
    }
  });

  function handleSubmit(e?: Event) {
    e?.preventDefault();
    // Review mode allows empty prompts; other modes require text
    if (!isReview && !prompt.trim()) return;
    if (starting) return;

    starting = true;
    onSubmit({ prompt: prompt.trim(), mode: currentMode, imageIds });
    // Close immediately; parent handles async start + optimistic timeline row.
    onClose({ prompt: '', mode: currentMode, imageIds: [] });
  }

  function handleClose() {
    onClose({ prompt, mode: currentMode, imageIds });
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

  function formatBaseBranch(baseBranch: string): string {
    return baseBranch.replace(/^origin\//, '');
  }

  // =========================================================================
  // Drag-and-drop images (via Tauri native drag-drop events)
  // =========================================================================

  async function handleFileDrop(paths: string[]) {
    const imagePaths = paths.filter((p) => isImageFile(p));
    const newIds: string[] = [];
    for (const path of imagePaths) {
      try {
        const image = await createImage(branch.id, branch.projectId, path, true);
        newIds.push(image.id);
      } catch (e) {
        console.error('Failed to create image from dropped file:', e);
      }
    }
    if (newIds.length > 0) {
      imageIds = [...imageIds, ...newIds];
      onImageIdsChange(imageIds);
    }
  }

  // Keep imageIds in sync with ImageAttachment changes
  function onImageIdsChange(ids: string[]) {
    imageIds = ids;
  }

  // Subscribe to the shared drag-drop service so the modal intercepts
  // drags that would otherwise land on the branch card behind it.
  $effect(() => {
    const el = modalElement;
    if (!el) return;

    const unsub = untrack(() =>
      subscribeDragDrop({
        element: el,
        blocking: true,
        onDragOver: (over) => {
          dragOver = over;
        },
        onDrop: (paths) => {
          handleFileDrop(paths);
        },
      })
    );

    return unsub;
  });
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onpointerdown={backdropDismiss.handlePointerDown}
  onclick={backdropDismiss.handleClick}
  onkeydown={(e) => e.key === 'Escape' && handleClose()}
>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    bind:this={modalElement}
    class="modal"
    class:drag-over={dragOver}
    role="presentation"
    onclick={(e) => e.stopPropagation()}
  >
    <header class="modal-header">
      <div class="header-title">
        {#if isReview}
          <span class="header-icon review-icon"><FileSearch size={14} /></span>
          <span>New code review</span>
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
        <span class="hint">{willQueue ? '⌘ Enter to queue' : '⌘ Enter to start'}</span>
      </div>

      <ImageAttachment
        branchId={branch.id}
        projectId={branch.projectId}
        disabled={starting}
        {imageIds}
        {onImageIdsChange}
      />

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
              {willQueue ? 'Queueing…' : 'Starting…'}
            {:else}
              <Send size={14} />
              {willQueue ? 'Queue' : 'Start'}
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
    border: 2px solid transparent;
    border-radius: 12px;
    overflow: hidden;
    box-shadow: var(--shadow-elevated);
    transition:
      border-color 0.15s,
      background-color 0.15s;
  }

  /* Drag-and-drop highlight */
  .modal.drag-over {
    border-color: var(--ui-accent);
    background-color: color-mix(in srgb, var(--ui-accent) 5%, var(--bg-chrome));
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
