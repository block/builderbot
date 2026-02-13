<!--
  BranchTimeline.svelte - Renders the unified timeline for a branch

  Receives timeline data, merges commits/notes/reviews by timestamp,
  and renders as a list of TimelineRow items.
  Pending items (running sessions, generating notes) appear at the bottom.
-->
<script lang="ts">
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
    onDeleteCommit?: (sha: string, sessionId?: string) => void;
    onDeletePendingCommit?: (commitId: string, sessionId?: string) => void;
    onDeleteNote?: (noteId: string, sessionId?: string) => void;
  }

  let {
    timeline,
    pendingDropNotes = [],
    onSessionClick,
    onCommitClick,
    onNoteClick,
    onDeleteCommit,
    onDeletePendingCommit,
    onDeleteNote,
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
    // Extra data for click handlers
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
      // Session finished but never produced a commit
      const isFailed = isPending && !isRunning && !!commit.sessionId;

      let type: TimelineItemType;
      let secondaryMeta: string | undefined;

      if (isFailed) {
        type = 'failed-commit';
        secondaryMeta = 'Session finished — no commit created';
      } else if (isPending || isRunning) {
        type = 'pending-commit';
        secondaryMeta = 'generating...';
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
      // Session finished but note has no real content
      const isFailed = !isRunning && !!note.sessionId && !note.content?.trim();

      let type: TimelineItemType;
      let secondaryMeta: string | undefined;

      if (isFailed) {
        type = 'failed-note';
        secondaryMeta = 'Session finished — no note created';
      } else if (isRunning) {
        type = 'generating-note';
        secondaryMeta = 'generating...';
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

    // Sort by timestamp ascending (oldest first), pending/failed items at bottom
    all.sort((a, b) => {
      const aIsTransient =
        a.type === 'pending-commit' ||
        a.type === 'generating-note' ||
        a.type === 'failed-commit' ||
        a.type === 'failed-note';
      const bIsTransient =
        b.type === 'pending-commit' ||
        b.type === 'generating-note' ||
        b.type === 'failed-commit' ||
        b.type === 'failed-note';
      if (aIsTransient !== bIsTransient) return aIsTransient ? 1 : -1;
      return a.timestamp - b.timestamp;
    });

    // Only the latest (HEAD) commit can be deleted via git reset.
    // Find the last completed commit and mark all others as non-deletable.
    let foundHead = false;
    for (let i = all.length - 1; i >= 0; i--) {
      if (all[i].type === 'commit') {
        if (!foundHead) {
          foundHead = true; // This is HEAD — leave deleteDisabledReason undefined
        } else {
          all[i].deleteDisabledReason = 'Only the latest commit can be deleted';
        }
      }
    }

    return all;
  });

  function handleItemClick(item: DisplayItem) {
    if (item.type === 'commit' && item.commitSha && onCommitClick) {
      onCommitClick(item.commitSha);
    } else if (item.type === 'note' && item.noteId && onNoteClick) {
      onNoteClick(item.noteId, item.noteTitle ?? '', item.noteContent ?? '');
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

{#if items.length === 0 && pendingDropNotes.length === 0}
  <p class="no-items">No commits or notes yet</p>
{:else}
  <div class="timeline">
    {#each items as item, index (item.key)}
      <TimelineRow
        type={item.type}
        title={item.title}
        meta={item.meta}
        secondaryMeta={item.secondaryMeta}
        isLast={index === items.length - 1 && pendingDropNotes.length === 0}
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
        isLast={index === pendingDropNotes.length - 1}
      />
    {/each}
  </div>
{/if}

<style>
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
</style>
