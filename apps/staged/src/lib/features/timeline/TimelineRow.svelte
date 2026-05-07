<!--
  TimelineRow.svelte - Renders a single timeline item (commit, note, or review)

  Icon + title + meta. Compact. The whole row is clickable to view the item.
  Hover reveals session and delete actions on the right.
-->
<script lang="ts">
  import {
    GitCommitVertical,
    FileDiff,
    FileText,
    FileSearch,
    Image as ImageLucide,
    MessageSquare,
    Trash2,
    AlertTriangle,
    Clock,
    GitBranch,
    GitMerge,
    ChevronsDown,
  } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';

  export type TimelineItemType =
    | 'commit'
    | 'pending-commit'
    | 'queued-commit'
    | 'failed-commit'
    | 'note'
    | 'generating-note'
    | 'queued-note'
    | 'failed-note'
    | 'review'
    | 'generating-review'
    | 'queued-review'
    | 'failed-review'
    | 'image'
    | 'git-info'
    | 'git-warning'
    | 'git-merge'
    | 'git-merge-warning'
    | 'git-pull'
    | 'git-push'
    | 'git-diff'
    | 'revalidating'
    | 'provisioning'
    | 'load-error';

  export type TimelineBadge = {
    icon: 'comment' | 'warning';
    count: number;
  };

  /** Data passed to the parent when the user right-clicks a row with context menu actions. */
  export type ContextMenuEvent = {
    x: number;
    y: number;
    commitSha?: string;
    hashtagRef?: string;
  };

  interface Props {
    type: TimelineItemType;
    title: string;
    /** Pre-rendered HTML title with hashtag badges. When set, takes precedence over `title`. */
    titleHtml?: string;
    meta?: string;
    secondaryMeta?: string;
    badges?: TimelineBadge[];
    deleting?: boolean;
    isLast?: boolean;
    sessionId?: string;
    onItemClick?: () => void;
    onSessionClick?: (sessionId: string) => void;
    onDeleteClick?: (opts?: { altKey: boolean }) => void;
    /** When set, the delete button is shown but disabled with this tooltip. */
    deleteDisabledReason?: string;
    onRetryClick?: () => void;
    onStartClick?: () => void;
    onResumeClick?: () => void;
    onPullClick?: () => void;
    pullDisabledReason?: string;
    onPushClick?: () => void;
    pushDisabledReason?: string;
    onRebaseClick?: () => void;
    rebaseDisabledReason?: string;
    onForcePushClick?: () => void;
    forcePushDisabledReason?: string;
    onViewDiffClick?: () => void;
    onCommitChangesClick?: () => void;
    commitChangesDisabledReason?: string;
    onDiscardChangesClick?: () => void;
    discardChangesDisabledReason?: string;
    showConnector?: boolean;
    /** Full commit SHA for the context menu "Copy SHA" action. */
    commitSha?: string;
    /** Hashtag reference token (e.g. "#commit:abc123") for "New session referring to this". */
    hashtagRef?: string;
    /** Callback when the user right-clicks and this row has context menu actions. */
    onContextMenu?: (event: ContextMenuEvent) => void;
  }

  let {
    type,
    title,
    titleHtml,
    meta,
    secondaryMeta,
    badges,
    deleting = false,
    isLast = false,
    sessionId,
    onItemClick,
    onSessionClick,
    onDeleteClick,
    deleteDisabledReason,
    onRetryClick,
    onStartClick,
    onResumeClick,
    onPullClick,
    pullDisabledReason,
    onPushClick,
    pushDisabledReason,
    onRebaseClick,
    rebaseDisabledReason,
    onForcePushClick,
    forcePushDisabledReason,
    onViewDiffClick,
    onCommitChangesClick,
    commitChangesDisabledReason,
    onDiscardChangesClick,
    discardChangesDisabledReason,
    showConnector = true,
    commitSha,
    hashtagRef,
    onContextMenu,
  }: Props = $props();

  let isNote = $derived(
    type === 'note' ||
      type === 'generating-note' ||
      type === 'queued-note' ||
      type === 'failed-note'
  );
  let isReview = $derived(
    type === 'review' ||
      type === 'generating-review' ||
      type === 'queued-review' ||
      type === 'failed-review'
  );
  let isImage = $derived(type === 'image');
  let isGitState = $derived(
    type === 'git-info' ||
      type === 'git-warning' ||
      type === 'git-merge' ||
      type === 'git-merge-warning' ||
      type === 'git-pull' ||
      type === 'git-push' ||
      type === 'git-diff'
  );
  let isQueued = $derived(
    type === 'queued-commit' || type === 'queued-note' || type === 'queued-review'
  );
  let isPending = $derived(
    deleting ||
      isQueued ||
      type === 'pending-commit' ||
      type === 'generating-note' ||
      type === 'generating-review' ||
      type === 'revalidating' ||
      type === 'provisioning'
  );
  let isFailed = $derived(
    !deleting &&
      (type === 'failed-commit' ||
        type === 'failed-note' ||
        type === 'failed-review' ||
        type === 'load-error')
  );
  let isClickable = $derived(!!onItemClick && !isPending && !isFailed);
  let hasSession = $derived(!!sessionId && !deleting);

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
    onDeleteClick?.({ altKey: e.altKey });
  }

  function handleRetryClick(e: MouseEvent) {
    e.stopPropagation();
    onRetryClick?.();
  }

  function handleStartClick(e: MouseEvent) {
    e.stopPropagation();
    onStartClick?.();
  }

  function handleResumeClick(e: MouseEvent) {
    e.stopPropagation();
    onResumeClick?.();
  }

  function handlePullClick(e: MouseEvent) {
    e.stopPropagation();
    onPullClick?.();
  }

  function handlePushClick(e: MouseEvent) {
    e.stopPropagation();
    onPushClick?.();
  }

  function handleRebaseClick(e: MouseEvent) {
    e.stopPropagation();
    onRebaseClick?.();
  }

  function handleForcePushClick(e: MouseEvent) {
    e.stopPropagation();
    onForcePushClick?.();
  }

  function handleViewDiffClick(e: MouseEvent) {
    e.stopPropagation();
    onViewDiffClick?.();
  }

  function handleCommitChangesClick(e: MouseEvent) {
    e.stopPropagation();
    onCommitChangesClick?.();
  }

  function handleDiscardChangesClick(e: MouseEvent) {
    e.stopPropagation();
    onDiscardChangesClick?.();
  }

  // ── Context menu ────────────────────────────────────────────────────
  let hasContextMenu = $derived(!!commitSha || !!hashtagRef);

  function handleContextMenu(e: MouseEvent) {
    if (!hasContextMenu || !onContextMenu) return;
    e.preventDefault();
    e.stopPropagation();
    onContextMenu({ x: e.clientX, y: e.clientY, commitSha, hashtagRef });
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="timeline-row"
  class:pending={isPending}
  class:failed={isFailed}
  class:clickable={isClickable}
  class:git-state={isGitState}
  class:compact={type === 'revalidating' || type === 'load-error'}
  onclick={handleRowClick}
  oncontextmenu={handleContextMenu}
>
  <div class="timeline-marker">
    <div
      class="timeline-icon"
      class:commit-icon={type === 'commit' || type === 'pending-commit' || type === 'queued-commit'}
      class:note-icon={type === 'note' || type === 'generating-note' || type === 'queued-note'}
      class:review-icon={type === 'review' ||
        type === 'generating-review' ||
        type === 'queued-review'}
      class:image-icon={isImage}
      class:branch-icon={isGitState}
      class:warning-icon={type === 'git-warning' || type === 'git-merge-warning'}
      class:failed-icon={isFailed}
    >
      {#if isQueued}
        <Clock size={12} />
      {:else if isPending}
        <Spinner size={12} />
      {:else if isFailed}
        <AlertTriangle size={12} />
      {:else if type === 'git-warning'}
        <AlertTriangle size={12} />
      {:else if type === 'git-merge' || type === 'git-merge-warning'}
        <GitMerge size={12} />
      {:else if type === 'git-pull'}
        <ChevronsDown size={12} />
      {:else if type === 'git-push'}
        <ChevronsDown size={12} />
      {:else if type === 'git-diff'}
        <FileDiff size={12} />
      {:else if type === 'commit'}
        <GitCommitVertical size={12} />
      {:else if isNote}
        <FileText size={12} />
      {:else if isReview}
        <FileSearch size={12} />
      {:else if isImage}
        <ImageLucide size={12} />
      {:else if isGitState}
        <GitBranch size={12} />
      {/if}
    </div>
    {#if showConnector && !isLast}
      <div class="timeline-line"></div>
    {/if}
  </div>
  <div class="timeline-content">
    <div class="timeline-info">
      {#if titleHtml}
        <span class="timeline-title" class:skeleton-title={isPending} class:failed-title={isFailed}
          >{@html titleHtml}</span
        >
      {:else}
        <span class="timeline-title" class:skeleton-title={isPending} class:failed-title={isFailed}
          >{title}</span
        >
      {/if}
      {#if meta || secondaryMeta || (badges && badges.length > 0)}
        <div class="timeline-meta">
          {#if meta}
            <span class="meta-item">{meta}</span>
          {/if}
          {#if secondaryMeta}
            <span class="meta-item meta-sha" class:failed-meta={isFailed}>{secondaryMeta}</span>
          {/if}
          {#if badges}
            {#each badges as badge}
              <span class="meta-badge">
                {#if badge.icon === 'warning'}
                  <AlertTriangle size={10} />
                {:else}
                  <MessageSquare size={10} />
                {/if}
                <span>{badge.count}</span>
              </span>
            {/each}
          {/if}
        </div>
      {/if}
    </div>
    <div
      class="timeline-actions"
      class:always-visible={!!onRetryClick ||
        !!onStartClick ||
        !!onResumeClick ||
        !!onPullClick ||
        !!pullDisabledReason ||
        !!onPushClick ||
        !!pushDisabledReason ||
        !!onRebaseClick ||
        !!rebaseDisabledReason ||
        !!onForcePushClick ||
        !!forcePushDisabledReason ||
        !!onViewDiffClick ||
        !!onCommitChangesClick ||
        !!commitChangesDisabledReason ||
        !!onDiscardChangesClick ||
        !!discardChangesDisabledReason}
    >
      {#if onStartClick}
        <button class="action-btn start-btn" onclick={handleStartClick} title="Start">
          Start
        </button>
      {/if}
      {#if onRetryClick}
        <button class="action-btn retry-btn" onclick={handleRetryClick} title="Retry">
          Retry
        </button>
      {/if}
      {#if onResumeClick}
        <button class="action-btn resume-btn" onclick={handleResumeClick} title="Resume session">
          Resume
        </button>
      {/if}
      {#if onPullClick || pullDisabledReason}
        <button
          class="action-btn resume-btn"
          onclick={handlePullClick}
          disabled={!!pullDisabledReason}
          title={pullDisabledReason ?? 'Pull'}
        >
          Pull
        </button>
      {/if}
      {#if onPushClick || pushDisabledReason}
        <button
          class="action-btn resume-btn"
          onclick={handlePushClick}
          disabled={!!pushDisabledReason}
          title={pushDisabledReason ?? 'Push'}
        >
          Push
        </button>
      {/if}
      {#if onForcePushClick || forcePushDisabledReason}
        <button
          class="action-btn danger-btn"
          onclick={handleForcePushClick}
          disabled={!!forcePushDisabledReason}
          title={forcePushDisabledReason ?? 'Force push local branch to origin'}
        >
          Force Push
        </button>
      {/if}
      {#if onRebaseClick || rebaseDisabledReason}
        <button
          class="action-btn resume-btn"
          onclick={handleRebaseClick}
          disabled={!!rebaseDisabledReason}
          title={rebaseDisabledReason ?? 'Rebase'}
        >
          Rebase
        </button>
      {/if}
      {#if onViewDiffClick}
        <button class="action-btn resume-btn" onclick={handleViewDiffClick} title="View diff">
          Diff
        </button>
      {/if}
      {#if onCommitChangesClick || commitChangesDisabledReason}
        <button
          class="action-btn resume-btn"
          onclick={handleCommitChangesClick}
          disabled={!!commitChangesDisabledReason}
          title={commitChangesDisabledReason ?? 'Commit changes'}
        >
          Commit
        </button>
      {/if}
      {#if onDiscardChangesClick || discardChangesDisabledReason}
        <button
          class="action-btn resume-btn"
          onclick={handleDiscardChangesClick}
          disabled={!!discardChangesDisabledReason}
          title={discardChangesDisabledReason ?? 'Discard changes'}
        >
          Discard
        </button>
      {/if}
      {#if hasSession && !onStartClick && !isQueued}
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
    position: relative;
    transition: background-color 0.15s ease;
    will-change: transform;
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

  .timeline-row.compact {
    padding: 6px 8px;
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

  .timeline-row.git-state .timeline-line {
    flex: none;
    height: 6px;
    min-height: 0;
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
    color: var(--review-color);
    background-color: var(--review-bg);
    border-color: transparent;
  }

  .timeline-icon.image-icon {
    color: var(--image-color);
    background-color: var(--image-bg);
    border-color: transparent;
  }

  .timeline-icon.branch-icon {
    color: var(--text-muted);
    background-color: var(--bg-hover);
    border-color: transparent;
  }

  .timeline-icon.warning-icon {
    color: var(--ui-danger);
    background-color: var(--ui-danger-bg);
    border-color: transparent;
  }

  .timeline-row.pending .timeline-icon.commit-icon {
    background-color: var(--commit-bg);
    border-color: transparent;
  }

  .timeline-row.pending .timeline-icon.commit-icon :global(.spinner) {
    color: var(--commit-color);
  }

  .timeline-row.pending .timeline-icon.note-icon {
    background-color: var(--note-bg);
    border-color: transparent;
  }

  .timeline-row.pending .timeline-icon.note-icon :global(.spinner) {
    color: var(--note-color);
  }

  .timeline-row.pending .timeline-icon.review-icon {
    background-color: var(--review-bg);
    border-color: transparent;
  }

  .timeline-row.pending .timeline-icon.review-icon :global(.spinner) {
    color: var(--review-color);
  }

  .timeline-row.compact .timeline-icon {
    background-color: var(--bg-hover);
    border-color: var(--bg-hover);
  }

  .timeline-row.compact .timeline-icon :global(.spinner) {
    color: var(--text-faint);
  }

  .timeline-row.compact .timeline-title {
    color: var(--text-faint);
    font-weight: normal;
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

  .timeline-row.git-state .timeline-title {
    color: var(--text-muted);
    font-weight: 400;
  }

  .timeline-title :global(.git-ref-badge) {
    display: inline;
    padding: 1px 6px;
    border: 1px solid var(--border-subtle);
    border-radius: 999px;
    background: var(--bg-hover);
    color: var(--text-primary);
    font: inherit;
    font-weight: 600;
    line-height: inherit;
    vertical-align: baseline;
  }

  .timeline-row.git-state .timeline-title :global(.git-ref-badge) {
    color: var(--text-muted);
    font-weight: inherit;
  }

  .skeleton-title {
    color: var(--text-muted);
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

  .meta-sha {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
  }

  .meta-badge {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 2px 7px;
    border-radius: 8px;
    background: none;
    border: 1px solid var(--border-subtle);
    color: var(--text-muted);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 600;
    line-height: 1;
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

  .timeline-row:hover .timeline-actions,
  .timeline-actions.always-visible {
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

  .delete-btn:disabled,
  .action-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .retry-btn,
  .start-btn,
  .resume-btn {
    width: auto;
    padding: 0 8px;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .resume-btn {
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    font-weight: 500;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s;
  }

  .resume-btn:hover {
    border-color: var(--border-muted);
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .danger-btn {
    width: auto;
    padding: 0 8px;
    font-size: var(--size-xs);
    font-weight: 500;
    border: 1px solid var(--ui-danger-bg, var(--ui-danger));
    border-radius: 6px;
    color: var(--ui-danger);
  }

  .danger-btn:not(:disabled):hover {
    background: var(--ui-danger-bg, rgba(255, 59, 48, 0.1));
    border-color: var(--ui-danger);
  }

  .start-btn {
    border: 1px solid var(--border-muted);
    border-radius: 4px;
  }

  .retry-btn:hover,
  .start-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
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
