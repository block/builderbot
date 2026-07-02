<script lang="ts">
  import AlertTriangle from '@lucide/svelte/icons/alert-triangle';
  import Bot from '@lucide/svelte/icons/bot';
  import Check from '@lucide/svelte/icons/check';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Copy from '@lucide/svelte/icons/copy';
  import MessageSquare from '@lucide/svelte/icons/message-square';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import Undo2 from '@lucide/svelte/icons/undo-2';
  import { Button } from '$lib/components/ui/button';
  import type { Comment, CommentSessionState } from '../../types';
  import { getCommentSessionDisplay } from './commentSessionDisplay';
  import { formatLineRange, truncateText } from './diffModalHelpers';

  interface Props {
    comments: Comment[];
    deletedComments: Comment[];
    selectedCommentId: string | null;
    copiedFeedback: boolean;
    onSelectComment: (comment: Comment) => void;
    onCopyAll: () => void;
    onDeleteAll: () => void;
    onDeleteComment: (commentId: string) => void;
    onRestoreComment: (commentId: string) => void;
    /** Note-session state for a comment; omit to fall back to the type icons. */
    commentNoteState?: (comment: Comment) => CommentSessionState;
    /** Commit-session state for a comment; omit to fall back to the type icons. */
    commentCommitState?: (comment: Comment) => CommentSessionState;
  }

  let {
    comments,
    deletedComments,
    selectedCommentId,
    copiedFeedback,
    onSelectComment,
    onCopyAll,
    onDeleteAll,
    onDeleteComment,
    onRestoreComment,
    commentNoteState,
    commentCommitState,
  }: Props = $props();

  let deletedExpanded = $state(false);

  function getFileName(path: string): string {
    return path.split('/').pop() || path;
  }
</script>

{#snippet commentItemContent(
  comment: Comment,
  noteState: CommentSessionState,
  commitState: CommentSessionState
)}
  {#if noteState !== 'idle' || commitState !== 'idle'}
    <!-- A launched note/commit session takes precedence over the agent and
         warning/message icons, mirroring the stateful inline-diff buttons. -->
    <span class="comment-session-badges">
      {#if noteState !== 'idle'}
        {@const noteDisplay = getCommentSessionDisplay('note', noteState, 'badge')}
        {@const NoteIcon = noteDisplay.icon}
        <span class="comment-session-badge note" title={noteDisplay.title}>
          <NoteIcon size={12} />
        </span>
      {/if}
      {#if commitState !== 'idle'}
        {@const commitDisplay = getCommentSessionDisplay('commit', commitState, 'badge')}
        {@const CommitIcon = commitDisplay.icon}
        <span class="comment-session-badge commit" title={commitDisplay.title}>
          <CommitIcon size={12} />
        </span>
      {/if}
    </span>
  {:else}
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
  {/if}
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
      <Button
        variant="ghost"
        size="icon"
        class={[
          'size-auto rounded-[3px] p-0.5 shadow-none hover:bg-[var(--bg-hover)] [&_svg]:!size-3',
          copiedFeedback
            ? 'text-[var(--status-added)] hover:text-[var(--status-added)]'
            : 'text-muted-foreground hover:text-foreground',
        ]}
        title={copiedFeedback ? 'Copied!' : 'Copy all comments'}
        aria-label={copiedFeedback ? 'Copied!' : 'Copy all comments'}
        onclick={onCopyAll}
      >
        {#if copiedFeedback}
          <Check size={12} />
        {:else}
          <Copy size={12} />
        {/if}
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="size-auto rounded-[3px] p-0.5 text-muted-foreground shadow-none hover:bg-[var(--bg-hover)] hover:text-destructive [&_svg]:!size-3"
        title="Delete all comments"
        aria-label="Delete all comments"
        onclick={onDeleteAll}
      >
        <Trash2 size={12} />
      </Button>
    {/if}
  </div>
</div>

{#if comments.length > 0}
  <ul class="tree-section comments-section">
    {#each comments as comment (comment.id)}
      {@const noteState = commentNoteState?.(comment) ?? 'idle'}
      {@const commitState = commentCommitState?.(comment) ?? 'idle'}
      <li class="tree-item-wrapper">
        <div class="comment-item-container group/comment">
          <button
            class="tree-item comment-item"
            class:selected={selectedCommentId === comment.id}
            style="padding-left: 8px"
            onclick={() => onSelectComment(comment)}
          >
            {@render commentItemContent(comment, noteState, commitState)}
          </button>
          <Button
            variant="ghost"
            size="icon"
            class="comment-row-action absolute top-1/2 right-3 z-10 size-auto -translate-y-1/2 rounded p-1 text-[var(--text-faint)] opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-primary)] hover:text-[var(--status-deleted)] group-hover/comment:opacity-100 [&_svg]:!size-3"
            title="Delete comment"
            aria-label="Delete comment"
            onclick={(e: MouseEvent) => {
              e.stopPropagation();
              onDeleteComment(comment.id);
            }}
          >
            <Trash2 size={12} />
          </Button>
        </div>
      </li>
    {/each}
  </ul>
{/if}

{#if deletedComments.length > 0}
  <Button
    variant="ghost"
    class="flex h-auto w-full items-center justify-start gap-1 rounded-none px-3 py-1 text-[length:calc(var(--size-xs)-1px)] font-semibold tracking-[0.03em] text-[var(--text-faint)] uppercase shadow-none hover:bg-[var(--bg-hover)] hover:text-muted-foreground"
    onclick={() => (deletedExpanded = !deletedExpanded)}
  >
    <span class="deleted-toggle-icon" class:expanded={deletedExpanded}>
      <ChevronRight size={12} />
    </span>
    <span>Deleted</span>
    <span class="count-capsule">{deletedComments.length}</span>
  </Button>

  {#if deletedExpanded}
    <ul class="tree-section comments-section deleted-comments-section">
      {#each deletedComments as comment (comment.id)}
        <li class="tree-item-wrapper">
          <div class="comment-item-container deleted-comment group/comment">
            <div class="tree-item comment-item" style="padding-left: 8px">
              <!-- Deleted comments come from a separate source and aren't
                   session-seeded, so they always show the type icons. -->
              {@render commentItemContent(comment, 'idle', 'idle')}
            </div>
            <Button
              variant="ghost"
              size="icon"
              class="comment-row-action absolute top-1/2 right-3 z-10 size-auto -translate-y-1/2 rounded p-1 text-[var(--text-faint)] opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-primary)] hover:text-[var(--status-added)] group-hover/comment:opacity-100 [&_svg]:!size-3"
              title="Restore comment"
              aria-label="Restore comment"
              onclick={(e: MouseEvent) => {
                e.stopPropagation();
                onRestoreComment(comment.id);
              }}
            >
              <Undo2 size={12} />
            </Button>
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

  .comment-item-container:focus-within :global(.comment-row-action),
  :global(.comment-row-action:focus-visible) {
    opacity: 1;
  }

  @media (hover: none), (pointer: coarse) {
    :global(.comment-row-action) {
      opacity: 1;
    }
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

  /* Session badges replace the icon gutter when a comment launched a note or
     commit session. Reuses the --note/--commit colours from the timeline icons
     (TimelineRow.svelte) and the shared Spinner. Offset left enough that two
     16px chips still clear the comment text when a comment spawned both. */
  .comment-session-badges {
    position: absolute;
    left: 4px;
    top: 6px;
    display: flex;
    align-items: center;
    gap: 3px;
  }

  .comment-session-badge {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .comment-session-badge.note {
    color: var(--note-color);
    background-color: var(--note-bg);
  }

  .comment-session-badge.commit {
    color: var(--commit-color);
    background-color: var(--commit-bg);
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

  /* Deleted comments section */

  .deleted-toggle-icon {
    display: flex;
    align-items: center;
    transition: transform 0.15s ease;
  }

  .deleted-toggle-icon.expanded {
    transform: rotate(90deg);
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
</style>
