<!--
  TimelineRow.svelte - Renders a single timeline item

  Supports: commit, note, pending commit, generating note, review.
  Icon + title + meta. Compact. The whole row is clickable to view the item.
  Hover reveals session and delete actions on the right.
-->
<script lang="ts">
  import {
    GitCommit,
    FileText,
    FileSearch,
    MessageSquare,
    Trash2,
    AlertTriangle,
  } from 'lucide-svelte';
  import Spinner from './Spinner.svelte';

  export type TimelineItemType =
    | 'commit'
    | 'pending-commit'
    | 'failed-commit'
    | 'note'
    | 'generating-note'
    | 'failed-note'
    | 'review';

  interface Props {
    type: TimelineItemType;
    title: string;
    meta?: string;
    secondaryMeta?: string;
    isLast?: boolean;
    sessionId?: string;
    onItemClick?: () => void;
    onSessionClick?: (sessionId: string) => void;
    onDeleteClick?: () => void;
    /** When set, the delete button is shown but disabled with this tooltip. */
    deleteDisabledReason?: string;
  }

  let {
    type,
    title,
    meta,
    secondaryMeta,
    isLast = false,
    sessionId,
    onItemClick,
    onSessionClick,
    onDeleteClick,
    deleteDisabledReason,
  }: Props = $props();

  let isPending = $derived(type === 'pending-commit' || type === 'generating-note');
  let isFailed = $derived(type === 'failed-commit' || type === 'failed-note');
  let isClickable = $derived(!!onItemClick && !isPending && !isFailed);
  let hasSession = $derived(!!sessionId);

  function handleRowClick() {
    if (isClickable) {
      onItemClick?.();
    }
  }

  function handleSessionClick(e: MouseEvent) {
    e.stopPropagation();
    if (sessionId && onSessionClick) {
      onSessionClick(sessionId);
    }
  }

  function handleDeleteClick(e: MouseEvent) {
    e.stopPropagation();
    onDeleteClick?.();
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="timeline-row"
  class:pending={isPending}
  class:failed={isFailed}
  class:clickable={isClickable}
  class:commit-row={type === 'commit' || type === 'pending-commit' || type === 'failed-commit'}
  class:note-row={type === 'note' || type === 'generating-note' || type === 'failed-note'}
  onclick={handleRowClick}
>
  <div class="timeline-marker">
    <div
      class="timeline-icon"
      class:commit-icon={type === 'commit' || type === 'pending-commit'}
      class:note-icon={type === 'note' || type === 'generating-note'}
      class:review-icon={type === 'review'}
      class:failed-icon={isFailed}
    >
      {#if type === 'pending-commit'}
        <Spinner size={12} />
      {:else if type === 'generating-note'}
        <Spinner size={12} />
      {:else if type === 'failed-commit' || type === 'failed-note'}
        <AlertTriangle size={12} />
      {:else if type === 'commit'}
        <GitCommit size={12} />
      {:else if type === 'note'}
        <FileText size={12} />
      {:else if type === 'review'}
        <FileSearch size={12} />
      {/if}
    </div>
    {#if !isLast}
      <div class="timeline-line"></div>
    {/if}
  </div>
  <div class="timeline-content">
    <div class="timeline-info">
      <span class="timeline-title" class:skeleton-title={isPending} class:failed-title={isFailed}
        >{title}</span
      >
      {#if meta || secondaryMeta}
        <div class="timeline-meta">
          {#if meta}
            <span class="meta-item">{meta}</span>
          {/if}
          {#if secondaryMeta}
            <span class="meta-item" class:failed-meta={isFailed}>{secondaryMeta}</span>
          {/if}
        </div>
      {/if}
    </div>
    <div class="timeline-actions">
      {#if hasSession}
        <button class="action-btn session-btn" onclick={handleSessionClick} title="View session">
          <MessageSquare size={12} />
        </button>
      {/if}
      {#if onDeleteClick || deleteDisabledReason}
        <button
          class="action-btn delete-btn"
          onclick={handleDeleteClick}
          disabled={!!deleteDisabledReason}
          title={deleteDisabledReason ?? 'Delete'}
        >
          <Trash2 size={12} />
        </button>
      {/if}
    </div>
  </div>
</div>

<style>
  .timeline-row {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 8px;
    margin: 0 -8px;
    border-radius: 6px;
    width: calc(100% + 16px);
    position: relative;
    transition: background-color 0.15s ease;
  }

  .timeline-row:hover {
    background-color: var(--bg-hover);
  }

  .timeline-row.clickable {
    cursor: pointer;
  }

  .timeline-row.pending {
    cursor: default;
  }

  .timeline-row.failed {
    cursor: default;
  }

  .timeline-marker {
    display: flex;
    flex-direction: column;
    align-items: center;
    width: 20px;
    flex-shrink: 0;
  }

  .timeline-line {
    flex: 1;
    width: 2px;
    min-height: 20px;
    background-color: var(--border-subtle);
    margin-top: 6px;
  }

  .timeline-content {
    flex: 1;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    min-width: 0;
  }

  .timeline-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: 4px;
    flex-shrink: 0;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
  }

  .timeline-icon.commit-icon {
    color: var(--commit-color);
    background-color: var(--commit-bg);
    border-color: transparent;
  }

  .timeline-icon.note-icon {
    color: var(--note-color);
    background-color: var(--note-bg);
    border-color: transparent;
  }

  .timeline-icon.review-icon {
    color: var(--status-modified);
  }

  /* Row background tints for commit/note distinction */
  .timeline-row.commit-row:hover {
    background-color: var(--commit-bg);
  }

  .timeline-row.note-row:hover {
    background-color: var(--note-bg);
  }

  .timeline-row.pending .timeline-icon {
    background-color: var(--bg-elevated);
    border-color: transparent;
  }

  .timeline-row.pending .timeline-icon :global(.spinner) {
    color: var(--text-primary);
  }

  .timeline-icon.failed-icon {
    color: var(--text-muted);
    border-color: var(--border-muted);
  }

  .timeline-info {
    flex: 1;
    min-width: 0;
  }

  .timeline-title {
    display: block;
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 1.4;
  }

  .skeleton-title {
    color: var(--text-muted);
    font-style: italic;
    font-weight: normal;
  }

  .failed-title {
    color: var(--text-muted);
    font-style: italic;
    font-weight: normal;
  }

  .failed-meta {
    color: var(--text-muted);
  }

  .timeline-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 3px;
  }

  .meta-item {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .meta-item:first-child {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
  }

  /* Actions container — visible on row hover */
  .timeline-actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex-shrink: 0;
    opacity: 0;
    transition: opacity 0.1s;
  }

  .timeline-row:hover .timeline-actions {
    opacity: 1;
  }

  .action-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .session-btn:hover {
    color: var(--ui-accent);
    background: var(--bg-hover);
  }

  .delete-btn:not(:disabled):hover {
    color: var(--ui-danger);
    background: var(--bg-hover);
  }

  .delete-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.5;
    }
  }
</style>
