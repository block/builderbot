<script lang="ts">
  import {
    AlertTriangle,
    Bot,
    Check,
    ChevronRight,
    Copy,
    FileText,
    GitCommitVertical,
    Github,
    MessageSquare,
    Trash2,
    Undo2,
  } from 'lucide-svelte';
  import type { Comment } from '../../types';
  import { formatLineRange, truncateText } from './diffModalHelpers';

  interface Props {
    comments: Comment[];
    deletedComments: Comment[];
    selectedCommentId: string | null;
    copiedFeedback: boolean;
    hasPr: boolean;
    onSelectComment: (comment: Comment) => void;
    onCopyAll: () => void;
    onDeleteAll: () => void;
    onDeleteComment: (commentId: string) => void;
    onRestoreComment: (commentId: string) => void;
    onNewNote: (comment: Comment, event: MouseEvent) => void;
    onNewCommit: (comment: Comment, event: MouseEvent) => void;
    onSendToGithub: (comment: Comment) => void;
  }

  let {
    comments,
    deletedComments,
    selectedCommentId,
    copiedFeedback,
    hasPr,
    onSelectComment,
    onCopyAll,
    onDeleteAll,
    onDeleteComment,
    onRestoreComment,
    onNewNote,
    onNewCommit,
    onSendToGithub,
  }: Props = $props();

  let deletedExpanded = $state(false);

  function getFileName(path: string): string {
    return path.split('/').pop() || path;
  }
</script>

{#snippet commentItemContent(comment: Comment)}
  <span class="comment-icons">
    {#if comment.author === 'agent'}
      <span class="comment-icon agent-icon">
        <Bot size={12} />
      </span>
    {/if}
    <span class="comment-icon" class:comment-icon-warning={comment.commentType === 'warning'}>
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
{/snippet}

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
            {@render commentItemContent(comment)}
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
        {#if selectedCommentId === comment.id}
          <div class="comment-actions">
            <button
              class="comment-action-btn note-btn"
              onclick={(e) => {
                e.stopPropagation();
                onNewNote(comment, e);
              }}
              title="New note (Option+click to skip dialog)"
            >
              <FileText size={12} />
              <span>Note</span>
            </button>
            <button
              class="comment-action-btn commit-btn"
              onclick={(e) => {
                e.stopPropagation();
                onNewCommit(comment, e);
              }}
              title="New commit (Option+click to skip dialog)"
            >
              <GitCommitVertical size={12} />
              <span>Commit</span>
            </button>
            {#if hasPr}
              <button
                class="comment-action-btn github-btn"
                onclick={(e) => {
                  e.stopPropagation();
                  onSendToGithub(comment);
                }}
                title="Send to GitHub"
              >
                <Github size={12} />
                <span>GitHub</span>
              </button>
            {/if}
          </div>
        {/if}
      </li>
    {/each}
  </ul>
{/if}

{#if deletedComments.length > 0}
  <button class="deleted-toggle" onclick={() => (deletedExpanded = !deletedExpanded)}>
    <span class="deleted-toggle-icon" class:expanded={deletedExpanded}>
      <ChevronRight size={12} />
    </span>
    <span class="deleted-toggle-label">Deleted</span>
    <span class="count-capsule">{deletedComments.length}</span>
  </button>

  {#if deletedExpanded}
    <ul class="tree-section comments-section deleted-comments-section">
      {#each deletedComments as comment (comment.id)}
        <li class="tree-item-wrapper">
          <div class="comment-item-container deleted-comment">
            <div class="tree-item comment-item" style="padding-left: 8px">
              {@render commentItemContent(comment)}
            </div>
            <button
              class="comment-restore-btn"
              onclick={(e) => {
                e.stopPropagation();
                onRestoreComment(comment.id);
              }}
              title="Restore comment"
            >
              <Undo2 size={12} />
            </button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
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
    color: var(--diff-comment-accent);
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

  /* Deleted comments section */

  .deleted-toggle {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 12px;
    background: none;
    border: none;
    color: var(--text-faint);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 600;
    letter-spacing: 0.03em;
    cursor: pointer;
    width: 100%;
    text-align: left;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .deleted-toggle:hover {
    color: var(--text-muted);
    background-color: var(--bg-hover);
  }

  .deleted-toggle-icon {
    display: flex;
    align-items: center;
    transition: transform 0.15s ease;
  }

  .deleted-toggle-icon.expanded {
    transform: rotate(90deg);
  }

  .deleted-toggle-label {
    text-transform: uppercase;
  }

  .deleted-comments-section {
    margin-bottom: 8px;
  }

  .deleted-comment {
    opacity: 0.5;
  }

  .deleted-comment:hover {
    opacity: 0.8;
  }

  .deleted-comment .tree-item {
    cursor: default;
  }

  .comment-restore-btn {
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

  .comment-item-container:hover .comment-restore-btn {
    opacity: 1;
  }

  .comment-restore-btn:hover {
    color: var(--status-added);
    background-color: var(--bg-primary);
  }

  /* Action buttons row */

  .comment-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px 6px 40px;
  }

  .comment-action-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 8px;
    border-radius: 5px;
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

  .comment-action-btn.github-btn:hover {
    color: var(--text-primary);
    border-color: var(--text-muted);
    background-color: var(--bg-hover);
  }
</style>
