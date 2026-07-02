<!--
  CommentEditor.svelte - Floating comment editor
  
  A positioned textarea for adding/editing comments on code ranges.
  Handles its own visibility based on scroll position.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import { Trash2 } from '@lucide/svelte';
  import type { Snippet } from 'svelte';
  import type { Comment, CommentActionContext } from '../types';
  import {
    createCommentAutosaveController,
    shouldDeleteCommentOnDismiss,
    type CommentSaveStatus,
  } from '../state/commentAutosave';

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
    /** Called when comment content should be persisted. */
    onSave: (commentId: string | null, content: string) => Promise<Comment | null | void>;
    /** Called when the editor should close after pending work is flushed. */
    onClose: () => void | Promise<void>;
    /** Called when comment is deleted (only shown if existingComment is set) */
    onDelete?: () => void | Promise<void>;
    /** Called instead of onDelete when an empty persisted comment is dismissed. */
    onDismissDelete?: () => void | Promise<void>;
    /** Host-rendered actions for existing comments. */
    commentActions?: Snippet<[CommentActionContext]>;
  }

  let {
    top,
    left,
    width,
    visible = true,
    existingComment = null,
    readOnly = false,
    placeholder = 'Add a comment...',
    onSave,
    onClose,
    onDelete,
    onDismissDelete,
    commentActions,
  }: Props = $props();

  // Track current input value - initialized by effect when existingComment changes
  let currentValue = $state('');
  let textareaEl: HTMLTextAreaElement | null = null;
  let persistedComment = $state<Comment | null>(null);
  let saveStatus = $state<CommentSaveStatus>('idle');
  let saveError = $state<unknown>(null);
  let lastExternalCommentId: string | null | undefined = undefined;
  let flushPendingOnDestroy = true;

  const autosave = createCommentAutosaveController({
    addComment: async (content) => {
      return (await onSave(null, content)) ?? null;
    },
    updateComment: async (commentId, content) => {
      await onSave(commentId, content);
    },
    onChange: ({ comment, status, error }) => {
      persistedComment = comment;
      saveStatus = status;
      saveError = error;
    },
  });

  // Update value when existingComment changes (for editing mode). Reset scroll
  // on identity change so a previously-scrolled textarea doesn't leak its
  // offset into the next comment being viewed.
  $effect(() => {
    const nextCommentId = existingComment?.id ?? null;

    if (nextCommentId === lastExternalCommentId) {
      if (existingComment) {
        autosave.updateExternalComment(existingComment);
      }
      return;
    }

    lastExternalCommentId = nextCommentId;

    if (existingComment && autosave.getSnapshot().comment?.id === existingComment.id) {
      autosave.updateExternalComment(existingComment);
      return;
    }

    currentValue = existingComment?.content ?? '';
    autosave.reset(existingComment, currentValue);
    if (textareaEl) textareaEl.scrollTop = 0;
  });

  onDestroy(() => {
    if (!flushPendingOnDestroy) {
      autosave.dispose();
      return;
    }

    void autosave.flush().finally(() => {
      autosave.dispose();
    });
  });

  export async function ensureSaved(): Promise<Comment | null> {
    if (readOnly) return persistedComment;
    return await autosave.flush();
  }

  export async function dismiss(): Promise<boolean> {
    if (readOnly) {
      flushPendingOnDestroy = false;
      autosave.dispose();
      return true;
    }

    const dismissDelete = onDismissDelete ?? onDelete;
    const snapshotBeforeFlush = autosave.getSnapshot();
    if (dismissDelete && shouldDeleteCommentOnDismiss(snapshotBeforeFlush.comment, currentValue)) {
      flushPendingOnDestroy = false;
      autosave.dispose();
      await dismissDelete();
      return true;
    }

    await autosave.flush();
    const snapshot = autosave.getSnapshot();
    if (snapshot.status === 'error') return false;

    flushPendingOnDestroy = false;
    autosave.dispose();

    if (shouldDeleteCommentOnDismiss(snapshot.comment, currentValue) && dismissDelete) {
      await dismissDelete();
    }

    return true;
  }

  export function getSaveStatus(): CommentSaveStatus {
    return saveStatus;
  }

  async function flushAndClose() {
    await onClose();
  }

  function handleInput(e: Event) {
    const target = e.target as HTMLTextAreaElement;
    currentValue = target.value;
    autosave.setContent(currentValue);
  }

  async function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      await flushAndClose();
    }
  }

  function handleDelete() {
    flushPendingOnDestroy = false;
    autosave.dispose();
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
    bind:this={textareaEl}
    class="comment-textarea"
    {placeholder}
    value={currentValue}
    readonly={readOnly}
    oninput={handleInput}
    onkeydown={handleKeydown}
    use:autoFocus
  ></textarea>
  {#if saveStatus === 'error' || (commentActions && (!readOnly || persistedComment)) || (persistedComment && onDelete)}
    <div class="comment-editor-hint">
      {#if saveStatus === 'error'}
        <span
          class="comment-editor-error"
          title={saveError instanceof Error ? saveError.message : undefined}
        >
          Unable to save
        </span>
      {/if}
      {#if commentActions && (!readOnly || persistedComment)}
        <div class="comment-action-buttons">
          {@render commentActions({ comment: persistedComment, ensureSaved, saveStatus })}
        </div>
      {/if}
      {#if persistedComment && onDelete}
        <button class="delete-comment-btn" onclick={handleDelete} title="Delete comment">
          <Trash2 size={12} />
        </button>
      {/if}
    </div>
  {/if}
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
    justify-content: flex-end;
    gap: 6px;
    padding: 4px 12px 8px;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .comment-editor-error {
    margin-right: auto;
    color: var(--status-deleted);
  }

  .comment-action-buttons {
    display: flex;
    align-items: center;
    gap: 4px;
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
