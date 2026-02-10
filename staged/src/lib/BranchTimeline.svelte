<!--
  BranchTimeline.svelte - Renders the unified timeline for a branch

  Receives timeline data, merges commits/notes/reviews by timestamp,
  and renders as a list of TimelineRow items.
  Pending items (running sessions, generating notes) appear at the bottom.
-->
<script lang="ts">
  import type { BranchTimeline as BranchTimelineData } from './types';
  import TimelineRow from './TimelineRow.svelte';
  import type { TimelineItemType } from './TimelineRow.svelte';

  interface Props {
    timeline: BranchTimelineData;
    onSessionClick?: (sessionId: string) => void;
    onCommitClick?: (sha: string) => void;
    onNoteClick?: (noteId: string, title: string, content: string) => void;
    onDeleteCommit?: (sha: string, sessionId?: string) => void;
    onDeleteNote?: (noteId: string, sessionId?: string) => void;
  }

  let {
    timeline,
    onSessionClick,
    onCommitClick,
    onNoteClick,
    onDeleteCommit,
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

      all.push({
        key: commit.sha || `pending-${commit.sessionId || commit.timestamp}`,
        type: isPending || isRunning ? 'pending-commit' : 'commit',
        title: stripXmlTags(commit.subject),
        meta: commit.shortSha || undefined,
        secondaryMeta: isPending ? 'generating...' : formatRelativeTime(commit.timestamp),
        timestamp: commit.timestamp,
        sessionId: commit.sessionId ?? undefined,
        commitSha: commit.sha || undefined,
      });
    }

    for (const note of timeline.notes) {
      const isGenerating = note.sessionStatus === 'running';

      all.push({
        key: `note-${note.id}`,
        type: isGenerating ? 'generating-note' : 'note',
        title: stripXmlTags(note.title),
        secondaryMeta: isGenerating ? 'generating...' : formatRelativeTimeMs(note.createdAt),
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

    // Sort by timestamp ascending (oldest first), pending items at bottom
    all.sort((a, b) => {
      const aIsPending = a.type === 'pending-commit' || a.type === 'generating-note';
      const bIsPending = b.type === 'pending-commit' || b.type === 'generating-note';
      if (aIsPending !== bIsPending) return aIsPending ? 1 : -1;
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
    } else if (item.type === 'note' && item.noteId && onDeleteNote) {
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

{#if items.length === 0}
  <p class="no-items">No commits or notes yet</p>
{:else}
  <div class="timeline">
    {#each items as item, index (item.key)}
      <TimelineRow
        type={item.type}
        title={item.title}
        meta={item.meta}
        secondaryMeta={item.secondaryMeta}
        isLast={index === items.length - 1}
        sessionId={item.sessionId}
        deleteDisabledReason={item.deleteDisabledReason}
        {onSessionClick}
        onItemClick={() => handleItemClick(item)}
        onDeleteClick={item.deleteDisabledReason ? undefined : () => handleDeleteClick(item)}
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
