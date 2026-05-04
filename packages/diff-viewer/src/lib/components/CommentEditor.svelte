<!--
  CommentEditor.svelte - Floating comment editor
  
  A positioned textarea for adding/editing comments on code ranges.
  Handles its own visibility based on scroll position.
-->
<script lang="ts">
  import { Check, FileText, GitCommitVertical, Github, Loader2, Trash2 } from 'lucide-svelte';
  import type { Comment } from '../types';

  export type GithubButtonState = 'idle' | 'sending' | 'sent' | 'stale';

  interface Props {
    /** Position relative to the viewer container */
    top: number;
    left: number;
    width: number;
    /** Whether the editor is visible (not scrolled out of view) */
    visible?: boolean;
    /** Existing comment to edit (null for new comment) */
    existingComment?: Comment | null;
    /** Read-only mode for non-editable comments (e.g. agent comments). */
    readOnly?: boolean;
    /** Placeholder text */
    placeholder?: string;
    /** Called when comment is submitted */
    onSubmit: (content: string) => void;
    /** Called when editing is cancelled */
    onCancel: () => void;
    /** Called when comment is deleted (only shown if existingComment is set) */
    onDelete?: () => void;
    /** Called when "Note" action is clicked (only shown if existingComment is set). */
    onNote?: (event: MouseEvent) => void;
    /** Called when "Commit" action is clicked (only shown if existingComment is set). */
    onCommit?: (event: MouseEvent) => void;
    /** Called when "GitHub" action is clicked (only shown if existingComment is set). */
    onGithub?: () => void;
    /** Current state of the GitHub send/update button. */
    githubState?: GithubButtonState;
  }

  let {
    top,
    left,
    width,
    visible = true,
    existingComment = null,
    readOnly = false,
    placeholder = 'Add a comment...',
    onSubmit,
    onCancel,
    onDelete,
    onNote,
    onCommit,
    onGithub,
    githubState = 'idle',
  }: Props = $props();

  // Track current input value - initialized by effect when existingComment changes
  let currentValue = $state('');

  // Update value when existingComment changes (for editing mode)
  $effect(() => {
    currentValue = existingComment?.content ?? '';
  });

  function handleInput(e: Event) {
    const target = e.target as HTMLTextAreaElement;
    currentValue = target.value;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onCancel();
    } else if (e.key === 'Enter' && !e.shiftKey && !readOnly) {
      e.preventDefault();
      e.stopPropagation();
      // Get value directly from event target as fallback
      const target = e.target as HTMLTextAreaElement;
      const content = (currentValue || target.value || '').trim();

      if (content) {
        onSubmit(content);
      } else {
        onCancel();
      }
    }
  }

  function handleDelete() {
    onDelete?.();
  }

  /**
   * Svelte action to auto-focus textarea.
   */
  function autoFocus(node: HTMLTextAreaElement) {
    node.focus();
  }
</script>

<div
  class="comment-editor line-comment-editor"
  class:comment-editor-hidden={!visible}
  style="top: {top}px; left: {left}px; width: {width}px;"
>
  <textarea
    class="comment-textarea"
    {placeholder}
    value={currentValue}
    readonly={readOnly}
    oninput={handleInput}
    onkeydown={handleKeydown}
    use:autoFocus
  ></textarea>
  <div class="comment-editor-hint">
    <span class="comment-editor-help">
      {readOnly ? 'Read-only · Esc to close' : 'Enter to save · Esc to cancel'}
    </span>
    {#if existingComment && (onNote || onCommit || onGithub)}
      <div class="comment-action-buttons">
        {#if onNote}
          <button
            class="comment-action-btn note-btn"
            onclick={(e) => onNote?.(e)}
            title="New note (Option+click to skip dialog)"
          >
            <FileText size={12} />
            <span>Note</span>
          </button>
        {/if}
        {#if onCommit}
          <button
            class="comment-action-btn commit-btn"
            onclick={(e) => onCommit?.(e)}
            title="New commit (Option+click to skip dialog)"
          >
            <GitCommitVertical size={12} />
            <span>Commit</span>
          </button>
        {/if}
        {#if onGithub}
          <button
            class="comment-action-btn github-btn"
            class:github-btn-sent={githubState === 'sent'}
            onclick={() => onGithub?.()}
            title={githubState === 'sent'
              ? 'Open GitHub comment'
              : githubState === 'stale'
                ? 'Update on GitHub'
                : 'Send to GitHub'}
            disabled={githubState === 'sending'}
          >
            {#if githubState === 'sending'}
              <Loader2 size={12} class="spinner" />
            {:else if githubState === 'sent'}
              <Check size={12} class="github-sent-check" />
              <Github size={12} />
            {:else}
              <Github size={12} />
            {/if}
            {#if githubState === 'stale'}
              <span>Update on GitHub</span>
            {:else}
              <span>GitHub</span>
            {/if}
          </button>
        {/if}
      </div>
    {/if}
    {#if existingComment && onDelete}
      <button class="delete-comment-btn" onclick={handleDelete} title="Delete comment">
        <Trash2 size={12} />
      </button>
    {/if}
  </div>
</div>

<style>
  .comment-editor {
    position: absolute;
    z-index: 100;
    display: flex;
    flex-direction: column;
    background-color: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    overflow: hidden;
    transition:
      opacity 0.15s ease,
      box-shadow 0.15s ease;
  }

  .comment-editor-hidden {
    opacity: 0.3;
    pointer-events: none;
  }

  .comment-textarea {
    width: 100%;
    height: 84px;
    padding: 10px 12px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-family: inherit;
    font-size: var(--size-sm);
    line-height: 1.5;
    resize: none;
    overflow-y: auto;
    user-select: text;
  }

  .comment-textarea:focus {
    outline: none;
  }

  .comment-textarea::placeholder {
    color: var(--text-faint);
  }

  .comment-editor-hint {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px 8px;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .comment-editor-help {
    margin-right: auto;
  }

  .comment-action-buttons {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .comment-action-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 6px;
    border-radius: 4px;
    border: 1px dashed var(--border-subtle);
    background: transparent;
    color: var(--text-muted);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
    cursor: pointer;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s;
  }

  .comment-action-btn.note-btn :global(svg) {
    color: var(--note-color);
  }

  .comment-action-btn.commit-btn :global(svg) {
    color: var(--commit-color);
  }

  .comment-action-btn.github-btn :global(svg) {
    color: var(--text-primary);
  }

  .comment-action-btn.note-btn:hover {
    color: var(--note-color);
    border-color: var(--note-color);
    background-color: var(--note-bg);
  }

  .comment-action-btn.commit-btn:hover {
    color: var(--commit-color);
    border-color: var(--commit-color);
    background-color: var(--commit-bg);
  }

  .comment-action-btn.github-btn:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--text-muted);
    background-color: var(--bg-hover);
  }

  .comment-action-btn.github-btn:disabled {
    cursor: default;
    opacity: 0.7;
  }

  .comment-action-btn.github-btn-sent {
    border-style: solid;
    color: var(--text-primary);
    border-color: var(--status-added, #3fb950);
  }

  .comment-action-btn.github-btn-sent :global(.github-sent-check) {
    color: var(--status-added, #3fb950) !important;
  }

  .comment-action-btn :global(.spinner) {
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

  .delete-comment-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .delete-comment-btn:hover {
    color: var(--status-deleted);
    background-color: var(--bg-hover);
  }
</style>
