<!--
  BranchTimeline.svelte - Renders the unified timeline for a branch

  Commits, notes, and reviews are merged by timestamp into a single linear list.
  Active pending items (running sessions, generating notes) appear at the bottom.
  Failed sessions appear in chronological order with completed items.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { FileText, GitCommitHorizontal } from 'lucide-svelte';
  import type { BranchTimeline as BranchTimelineData } from '../../types';
  import TimelineRow from './TimelineRow.svelte';
  import type { TimelineItemType } from './TimelineRow.svelte';

  interface Props {
    timeline: BranchTimelineData;
    /** Placeholder items for notes being created from drag-and-drop. */
    pendingDropNotes?: { key: string; title: string }[];
    onSessionClick?: (sessionId: string) => void;
    onCommitClick?: (sha: string) => void;
    onNoteClick?: (noteId: string, title: string, content: string) => void;
    onReviewClick?: () => void;
    onDeleteCommit?: (sha: string, sessionId?: string) => void;
    onDeletePendingCommit?: (commitId: string, sessionId?: string) => void;
    onDeleteNote?: (noteId: string, sessionId?: string) => void;
    onNewNote?: () => void;
    onNewCommit?: () => void;
    newSessionDisabled?: boolean;
    footerActions?: Snippet;
  }

  let {
    timeline,
    pendingDropNotes = [],
    onSessionClick,
    onCommitClick,
    onNoteClick,
    onReviewClick,
    onDeleteCommit,
    onDeletePendingCommit,
    onDeleteNote,
    onNewNote,
    onNewCommit,
    newSessionDisabled = false,
    footerActions,
  }: Props = $props();

  // Unified timeline item for display
  type DisplayItem = {
    key: string;
    type: TimelineItemType;
    title: string;
    meta?: string;
    secondaryMeta?: string;
    timestamp: number;
    sessionId?: string;
    commitSha?: string;
    commitId?: string;
    noteId?: string;
    noteTitle?: string;
    noteContent?: string;
    /** When set, delete button is shown but disabled with this tooltip. */
    deleteDisabledReason?: string;
  };

  /** Strip XML-tagged context blocks (action, branch-history) from display text. */
  function stripXmlTags(text: string): string {
    return text.replace(/<(action|branch-history)>[\s\S]*?<\/\1>/g, '').trim();
  }

  // Merge commits, notes, and reviews into a single sorted list
  let items = $derived.by(() => {
    const all: DisplayItem[] = [];

    for (const commit of timeline.commits) {
      const isPending = !commit.sha;
      const isRunning = commit.sessionStatus === 'running';
      const isFailed = isPending && !isRunning && !!commit.sessionId;

      let type: TimelineItemType;
      let secondaryMeta: string | undefined;

      if (isFailed) {
        type = 'failed-commit';
        secondaryMeta = 'Session finished — no commit created';
      } else if (isPending || isRunning) {
        type = 'pending-commit';
        secondaryMeta = 'Generating commit';
      } else {
        type = 'commit';
        secondaryMeta = formatRelativeTime(commit.timestamp);
      }

      all.push({
        key: commit.sha || `pending-${commit.sessionId || commit.timestamp}`,
        type,
        title: stripXmlTags(commit.subject),
        meta: commit.shortSha || undefined,
        secondaryMeta,
        timestamp: commit.timestamp,
        sessionId: commit.sessionId ?? undefined,
        commitSha: commit.sha || undefined,
        commitId: commit.id ?? undefined,
      });
    }

    for (const note of timeline.notes) {
      const isRunning = note.sessionStatus === 'running';
      const isFailed = !isRunning && !!note.sessionId && !note.content?.trim();

      let type: TimelineItemType;
      let secondaryMeta: string | undefined;

      if (isFailed) {
        type = 'failed-note';
        secondaryMeta = 'Session finished — no note created';
      } else if (isRunning) {
        type = 'generating-note';
        secondaryMeta = 'Generating note';
      } else {
        type = 'note';
        secondaryMeta = formatRelativeTimeMs(note.createdAt);
      }

      all.push({
        key: `note-${note.id}`,
        type,
        title: stripXmlTags(note.title),
        secondaryMeta,
        // Note timestamps are in milliseconds, convert to seconds for sorting
        timestamp: Math.floor(note.createdAt / 1000),
        sessionId: note.sessionId ?? undefined,
        noteId: note.id,
        noteTitle: stripXmlTags(note.title),
        noteContent: note.content,
      });
    }

    for (const review of timeline.reviews) {
      all.push({
        key: `review-${review.id}`,
        type: 'review',
        title: `Code Review (${review.scope})`,
        meta: review.commentCount > 0 ? `${review.commentCount} comments` : undefined,
        secondaryMeta: formatRelativeTimeMs(review.createdAt),
        timestamp: Math.floor(review.createdAt / 1000),
        sessionId: review.sessionId ?? undefined,
      });
    }

    // Sort by timestamp ascending; pending/generating items at bottom
    all.sort((a, b) => {
      const aIsTransient = a.type === 'pending-commit' || a.type === 'generating-note';
      const bIsTransient = b.type === 'pending-commit' || b.type === 'generating-note';
      if (aIsTransient !== bIsTransient) return aIsTransient ? 1 : -1;
      return a.timestamp - b.timestamp;
    });

    // Only the latest (HEAD) commit can be deleted via git reset.
    let foundHead = false;
    for (let i = all.length - 1; i >= 0; i--) {
      if (all[i].type === 'commit') {
        if (!foundHead) {
          foundHead = true;
        } else {
          all[i].deleteDisabledReason = 'Only the latest commit can be deleted';
        }
      }
    }

    return all;
  });

  // ── Handlers ──────────────────────────────────────────────────────────

  function handleItemClick(item: DisplayItem) {
    if (item.type === 'commit' && item.commitSha && onCommitClick) {
      onCommitClick(item.commitSha);
    } else if (item.type === 'note' && item.noteId && onNoteClick) {
      onNoteClick(item.noteId, item.noteTitle ?? '', item.noteContent ?? '');
    } else if (item.type === 'review' && onReviewClick) {
      onReviewClick();
    }
  }

  function handleDeleteClick(item: DisplayItem) {
    if (item.type === 'commit' && item.commitSha && onDeleteCommit) {
      onDeleteCommit(item.commitSha, item.sessionId);
    } else if (
      (item.type === 'failed-commit' || item.type === 'pending-commit') &&
      item.commitId &&
      onDeletePendingCommit
    ) {
      onDeletePendingCommit(item.commitId, item.sessionId);
    } else if (
      (item.type === 'note' || item.type === 'failed-note' || item.type === 'generating-note') &&
      item.noteId &&
      onDeleteNote
    ) {
      onDeleteNote(item.noteId, item.sessionId);
    }
  }

  function formatRelativeTime(timestamp: number): string {
    const date = new Date(timestamp * 1000);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    const diffHours = Math.floor(diffMins / 60);
    const diffDays = Math.floor(diffHours / 24);

    if (diffMins < 1) return 'just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHours < 24) return `${diffHours}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;
    return date.toLocaleDateString();
  }

  function formatRelativeTimeMs(timestamp: number): string {
    return formatRelativeTime(Math.floor(timestamp / 1000));
  }
</script>

{#if items.length === 0 && !onNewNote && !onNewCommit && pendingDropNotes.length === 0}
  <p class="no-items">No commits or notes yet</p>
{:else if items.length === 0}
  <!-- Empty state: large action buttons -->
  <div class="empty-state">
    {#if onNewNote}
      <button
        class="empty-action-btn note-action"
        onclick={onNewNote}
        disabled={newSessionDisabled}
      >
        <FileText size={18} />
        <span>New note</span>
      </button>
    {/if}
    {#if onNewCommit}
      <button
        class="empty-action-btn commit-action"
        onclick={onNewCommit}
        disabled={newSessionDisabled}
      >
        <GitCommitHorizontal size={18} />
        <span>New commit</span>
      </button>
    {/if}
  </div>
{:else}
  <!-- Unified timeline (vertical) -->
  <div class="timeline">
    {#each items as item, index (item.key)}
      <TimelineRow
        type={item.type}
        title={item.title}
        meta={item.meta}
        secondaryMeta={item.secondaryMeta}
        isLast={index === items.length - 1 &&
          !onNewNote &&
          !onNewCommit &&
          pendingDropNotes.length === 0}
        sessionId={item.sessionId}
        deleteDisabledReason={item.deleteDisabledReason}
        {onSessionClick}
        onItemClick={() => handleItemClick(item)}
        onDeleteClick={item.deleteDisabledReason ? undefined : () => handleDeleteClick(item)}
      />
    {/each}
    {#each pendingDropNotes as drop, index (drop.key)}
      <TimelineRow
        type="generating-note"
        title={drop.title}
        secondaryMeta="adding..."
        isLast={index === pendingDropNotes.length - 1 && !onNewNote && !onNewCommit}
      />
    {/each}
    {#if onNewNote || onNewCommit || footerActions}
      <div class="footer-row">
        {#if onNewNote}
          <button
            class="add-item-btn note-btn"
            onclick={onNewNote}
            disabled={newSessionDisabled}
            title="New note"
          >
            <FileText size={13} />
            <span>New note</span>
          </button>
        {/if}
        {#if onNewCommit}
          <button
            class="add-item-btn commit-btn"
            onclick={onNewCommit}
            disabled={newSessionDisabled}
            title="New commit"
          >
            <GitCommitHorizontal size={13} />
            <span>New commit</span>
          </button>
        {/if}
        {#if footerActions}
          {@render footerActions()}
        {/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  /* ── Timeline ────────────────────────────────────────────────────────── */

  .timeline {
    display: flex;
    flex-direction: column;
  }

  .no-items {
    margin: 0;
    padding: 8px 0;
    font-size: var(--size-sm);
    color: var(--text-faint);
    font-style: italic;
    text-align: center;
  }

  /* ── Empty state ────────────────────────────────────────────────────── */

  .empty-state {
    display: flex;
    gap: 10px;
    padding: 4px 0;
  }

  .empty-action-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    flex: 1;
    padding: 10px 6px;
    border-radius: 8px;
    border: none;
    background: var(--bg-elevated);
    color: var(--text-muted);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition:
      color 0.15s,
      background-color 0.15s;
  }

  .empty-action-btn.note-action:hover:not(:disabled) {
    color: var(--note-color);
    background-color: var(--note-bg);
  }

  .empty-action-btn.commit-action:hover:not(:disabled) {
    color: var(--commit-color);
    background-color: var(--commit-bg);
  }

  .empty-action-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  /* ── Footer row with inline add buttons ─────────────────────────────── */

  .footer-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    margin: 0 -8px;
    position: relative;
    z-index: 1;
  }

  .add-item-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 4px 10px;
    border-radius: 6px;
    border: 1px dashed var(--border-subtle);
    background: none;
    color: var(--text-faint);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s;
  }

  .add-item-btn.note-btn:hover:not(:disabled) {
    color: var(--note-color);
    border-color: var(--note-color);
    background-color: var(--note-bg);
  }

  .add-item-btn.commit-btn:hover:not(:disabled) {
    color: var(--commit-color);
    border-color: var(--commit-color);
    background-color: var(--commit-bg);
  }

  .add-item-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }
</style>
