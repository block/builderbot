<!--
  CommentEditor.svelte - Floating comment editor
  
  A positioned textarea for adding/editing comments on code ranges.
  Handles its own visibility based on scroll position.
-->
<script lang="ts">
  import { Trash2 } from 'lucide-svelte';
  import type { Snippet } from 'svelte';
  import type { Comment, CommentActionContext } from '../types';

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
    onSubmit,
    onCancel,
    onDelete,
    commentActions,
  }: Props = $props();

  // Track current input value - initialized by effect when existingComment changes
  let currentValue = $state('');
  let textareaEl: HTMLTextAreaElement | null = null;

  // Update value when existingComment changes (for editing mode). Reset scroll
  // on identity change so a previously-scrolled textarea doesn't leak its
  // offset into the next comment being viewed.
  $effect(() => {
    existingComment?.id;
    currentValue = existingComment?.content ?? '';
    if (textareaEl) textareaEl.scrollTop = 0;
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
    bind:this={textareaEl}
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
    {#if existingComment && commentActions}
      <div class="comment-action-buttons">
        {@render commentActions({ comment: existingComment })}
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
