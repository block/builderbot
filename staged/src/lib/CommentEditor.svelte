<!--
  CommentEditor.svelte - Floating comment editor
  
  A positioned textarea for adding/editing comments on code ranges.
  Handles its own visibility based on scroll position.
-->
<script lang="ts">
  import { Trash2 } from 'lucide-svelte';
  import type { Comment } from './types';

  interface Props {
    /** Position relative to the viewer container */
    top: number;
    left: number;
    width: number;
    /** Whether the editor is visible (not scrolled out of view) */
    visible?: boolean;
    /** Existing comment to edit (null for new comment) */
    existingComment?: Comment | null;
    /** Placeholder text */
    placeholder?: string;
    /** Called when comment is submitted */
    onSubmit: (content: string) => void;
    /** Called when editing is cancelled */
    onCancel: () => void;
    /** Called when comment is deleted (only shown if existingComment is set) */
    onDelete?: () => void;
  }

  let {
    top,
    left,
    width,
    visible = true,
    existingComment = null,
    placeholder = 'Add a comment...',
    onSubmit,
    onCancel,
    onDelete,
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
    } else if (e.key === 'Enter' && !e.shiftKey) {
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
    oninput={handleInput}
    onkeydown={handleKeydown}
    use:autoFocus
  ></textarea>
  <div class="comment-editor-hint">
    <span>Enter to save · Esc to cancel</span>
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
    border-radius: 8px;
    overflow: hidden;
    transition: opacity 0.15s ease;
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
    padding: 4px 12px 8px;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .delete-comment-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    margin-left: auto;
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
