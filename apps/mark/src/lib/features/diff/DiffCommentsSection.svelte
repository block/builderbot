<script lang="ts">
  import { AlertTriangle, Bot, Check, Copy, MessageSquare, Trash2 } from 'lucide-svelte';
  import type { Comment } from '../../types';
  import { formatLineRange, truncateText } from './diffModalHelpers';

  interface Props {
    comments: Comment[];
    selectedCommentId: string | null;
    copiedFeedback: boolean;
    onSelectComment: (comment: Comment) => void;
    onCopyAll: () => void;
    onDeleteAll: () => void;
    onDeleteComment: (commentId: string) => void;
  }

  let {
    comments,
    selectedCommentId,
    copiedFeedback,
    onSelectComment,
    onCopyAll,
    onDeleteAll,
    onDeleteComment,
  }: Props = $props();

  function getFileName(path: string): string {
    return path.split('/').pop() || path;
  }
</script>

<div class="section-header comments-header">
  <div class="section-left"></div>
  <div class="section-divider">
    <span class="divider-label">COMMENTS</span>
    {#if comments.length > 0}
      <span class="count-capsule">{comments.length}</span>
    {/if}
  </div>
  <div class="section-right">
    {#if comments.length > 0}
      <button
        class="copy-btn"
        class:copied={copiedFeedback}
        onclick={onCopyAll}
        title="Copy all comments"
      >
        {#if copiedFeedback}
          <Check size={12} />
        {:else}
          <Copy size={12} />
        {/if}
      </button>
      <button class="delete-all-btn" onclick={onDeleteAll} title="Delete all comments">
        <Trash2 size={12} />
      </button>
    {/if}
  </div>
</div>

{#if comments.length > 0}
  <ul class="tree-section comments-section">
    {#each comments as comment (comment.id)}
      <li class="tree-item-wrapper">
        <div class="comment-item-container">
          <button
            class="tree-item comment-item"
            class:selected={selectedCommentId === comment.id}
            style="padding-left: 8px"
            onclick={() => onSelectComment(comment)}
          >
            <span class="comment-icons">
              {#if comment.author === 'agent'}
                <span class="comment-icon agent-icon">
                  <Bot size={12} />
                </span>
              {/if}
              <span
                class="comment-icon"
                class:comment-icon-warning={comment.commentType === 'warning'}
              >
                {#if comment.commentType === 'warning'}
                  <AlertTriangle size={12} />
                {:else}
                  <MessageSquare size={12} />
                {/if}
              </span>
            </span>
            <span class="comment-details">
              <span class="comment-location">
                <span class="comment-file">{getFileName(comment.path)}</span>
                <span class="comment-line">{formatLineRange(comment.span)}</span>
              </span>
              <span class="comment-preview">{truncateText(comment.content)}</span>
            </span>
          </button>
          <button
            class="comment-delete-btn"
            onclick={(e) => {
              e.stopPropagation();
              onDeleteComment(comment.id);
            }}
            title="Delete comment"
          >
            <Trash2 size={12} />
          </button>
        </div>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .section-header {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    margin: 16px 12px 8px;
    gap: 6px;
  }

  .section-left {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    min-height: 1px;
  }

  .section-left::after {
    content: '';
    display: block;
    width: 100%;
    border-top: 1px solid var(--bg-hover);
  }

  .section-right {
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: 4px;
    min-height: 1px;
  }

  .section-right::before {
    content: '';
    display: block;
    width: 100%;
    border-top: 1px solid var(--bg-hover);
  }

  .section-divider {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
  }

  .divider-label {
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 700;
    letter-spacing: 0.05em;
    color: var(--text-faint);
    text-transform: uppercase;
  }

  .count-capsule {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: 10px;
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 700;
    background-color: var(--bg-hover);
    color: var(--text-faint);
  }

  .tree-section {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .tree-item-wrapper {
    margin: 0;
  }

  .tree-item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 6px;
    border: none;
    background: none;
    color: var(--text-muted);
    font-size: var(--size-sm);
    text-align: left;
    cursor: pointer;
    transition:
      background-color 0.08s,
      color 0.08s;
    min-height: 24px;
    border-radius: 0;
  }

  .tree-item:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .tree-item.selected {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .comments-section {
    margin-bottom: 8px;
  }

  .comment-item-container {
    position: relative;
    width: 100%;
  }

  .comment-item {
    position: relative;
    flex-direction: column;
    align-items: flex-start !important;
    gap: 2px !important;
    padding-top: 6px !important;
    padding-bottom: 6px !important;
    padding-left: 40px !important;
    width: 100%;
  }

  .comment-icons {
    position: absolute;
    left: 8px;
    top: 8px;
    display: flex;
    align-items: center;
    gap: 3px;
  }

  .comment-icon {
    display: flex;
    align-items: center;
    color: var(--text-faint);
  }

  .comment-icon.agent-icon {
    color: var(--text-faint);
  }

  .comment-icon.comment-icon-warning {
    color: var(--status-modified);
  }

  .comment-details {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    min-width: 0;
    padding-right: 32px;
  }

  .comment-location {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-xs);
  }

  .comment-file {
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .comment-line {
    flex-shrink: 0;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-faint);
  }

  .comment-preview {
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .comment-delete-btn {
    position: absolute;
    right: 12px;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    opacity: 0;
    transition:
      opacity 0.1s,
      color 0.1s,
      background-color 0.1s;
    z-index: 1;
  }

  .comment-item-container:hover .comment-delete-btn {
    opacity: 1;
  }

  .comment-delete-btn:hover {
    color: var(--status-deleted);
    background-color: var(--bg-primary);
  }

  .copy-btn,
  .delete-all-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .copy-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .copy-btn.copied {
    color: var(--status-added);
  }

  .delete-all-btn:hover {
    background-color: var(--bg-hover);
    color: var(--status-deleted);
  }
</style>
