<!--
  BranchTimeline.svelte - Renders the timeline for a branch

  Notes are displayed as a horizontal strip of chips at the top.
  Commits and reviews are rendered below as a vertical timeline.
  Pending items (running sessions, generating notes) appear at the end of each section.
-->
<script lang="ts">
  import {
    FileText,
    GitCommitHorizontal,
    MessageSquare,
    Plus,
    Trash2,
    AlertTriangle,
  } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import type { BranchTimeline as BranchTimelineData } from '../../types';
  import TimelineRow from './TimelineRow.svelte';
  import type { TimelineItemType } from './TimelineRow.svelte';

  interface Props {
    timeline: BranchTimelineData;
    onSessionClick?: (sessionId: string) => void;
    onCommitClick?: (sha: string) => void;
    onNoteClick?: (noteId: string, title: string, content: string) => void;
    onDeleteCommit?: (sha: string, sessionId?: string) => void;
    onDeletePendingCommit?: (commitId: string, sessionId?: string) => void;
    onDeleteNote?: (noteId: string, sessionId?: string) => void;
    onNewNote?: () => void;
    onNewCommit?: () => void;
    newSessionDisabled?: boolean;
  }

  let {
    timeline,
    onSessionClick,
    onCommitClick,
    onNoteClick,
    onDeleteCommit,
    onDeletePendingCommit,
    onDeleteNote,
    onNewNote,
    onNewCommit,
    newSessionDisabled = false,
  }: Props = $props();

  // Display item shared by both sections
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

  // ── Notes (horizontal strip) ──────────────────────────────────────────

  type NoteItemType = 'note' | 'generating-note' | 'failed-note';

  let noteItems = $derived.by(() => {
    const notes: DisplayItem[] = [];

    for (const note of timeline.notes) {
      const isRunning = note.sessionStatus === 'running';
      const isFailed = !isRunning && !!note.sessionId && !note.content?.trim();

      let type: NoteItemType;
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

      notes.push({
        key: `note-${note.id}`,
        type,
        title: stripXmlTags(note.title),
        secondaryMeta,
        timestamp: Math.floor(note.createdAt / 1000),
        sessionId: note.sessionId ?? undefined,
        noteId: note.id,
        noteTitle: stripXmlTags(note.title),
        noteContent: note.content,
      });
    }

    // Sort: pending/failed at end, then by timestamp ascending
    notes.sort((a, b) => {
      const aTransient = a.type !== 'note';
      const bTransient = b.type !== 'note';
      if (aTransient !== bTransient) return aTransient ? 1 : -1;
      return a.timestamp - b.timestamp;
    });

    return notes;
  });

  // ── Commits & reviews (vertical timeline) ─────────────────────────────

  let timelineItems = $derived.by(() => {
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

    // Sort: pending/failed at end, then by timestamp ascending
    all.sort((a, b) => {
      const aTransient = a.type === 'pending-commit' || a.type === 'failed-commit';
      const bTransient = b.type === 'pending-commit' || b.type === 'failed-commit';
      if (aTransient !== bTransient) return aTransient ? 1 : -1;
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

  function handleCommitItemClick(item: DisplayItem) {
    if (item.type === 'commit' && item.commitSha && onCommitClick) {
      onCommitClick(item.commitSha);
    }
  }

  function handleNoteChipClick(item: DisplayItem) {
    if (item.type === 'note' && item.noteId && onNoteClick) {
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

  function handleNoteSessionClick(e: MouseEvent, sessionId: string) {
    e.stopPropagation();
    onSessionClick?.(sessionId);
  }

  function handleNoteDeleteClick(e: MouseEvent, item: DisplayItem) {
    e.stopPropagation();
    handleDeleteClick(item);
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

{#if noteItems.length === 0 && timelineItems.length === 0 && !onNewNote && !onNewCommit}
  <p class="no-items">No commits or notes yet</p>
{:else}
  <!-- Notes strip (horizontal) -->
  {#if noteItems.length > 0 || onNewNote}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="notes-strip">
      {#each noteItems as note (note.key)}
        {@const isPending = note.type === 'generating-note'}
        {@const isFailed = note.type === 'failed-note'}
        {@const isClickable = note.type === 'note'}
        <div
          class="note-chip"
          class:pending={isPending}
          class:failed={isFailed}
          class:clickable={isClickable}
          title={isFailed ? (note.secondaryMeta ?? '') : note.title}
          onclick={() => handleNoteChipClick(note)}
        >
          <span class="note-chip-icon">
            {#if isPending}
              <Spinner size={14} />
            {:else if isFailed}
              <AlertTriangle size={14} />
            {:else}
              <FileText size={14} />
            {/if}
          </span>
          <span class="note-chip-info">
            <span class="note-chip-title">{isPending ? 'generating...' : note.title}</span>
            {#if note.secondaryMeta}
              <span class="note-chip-time">{note.secondaryMeta}</span>
            {/if}
          </span>
          <span class="note-chip-actions">
            {#if note.sessionId}
              <button
                class="chip-action-btn session-btn"
                onclick={(e) => handleNoteSessionClick(e, note.sessionId!)}
                title="View session"
              >
                <MessageSquare size={10} />
              </button>
            {/if}
            <button
              class="chip-action-btn delete-btn"
              onclick={(e) => handleNoteDeleteClick(e, note)}
              title="Delete"
            >
              <Trash2 size={10} />
            </button>
          </span>
        </div>
      {/each}
      {#if onNewNote}
        <button
          class="add-note-btn"
          onclick={onNewNote}
          disabled={newSessionDisabled}
          title="New note"
        >
          <Plus size={13} />
          <span>New note</span>
        </button>
      {/if}
    </div>
  {/if}

  <!-- Commit timeline (vertical) -->
  {#if timelineItems.length > 0 || onNewCommit}
    <div class="timeline" class:has-notes={noteItems.length > 0 || !!onNewNote}>
      {#each timelineItems as item, index (item.key)}
        <TimelineRow
          type={item.type}
          title={item.title}
          meta={item.meta}
          secondaryMeta={item.secondaryMeta}
          isLast={index === timelineItems.length - 1 && !onNewCommit}
          sessionId={item.sessionId}
          deleteDisabledReason={item.deleteDisabledReason}
          {onSessionClick}
          onItemClick={() => handleCommitItemClick(item)}
          onDeleteClick={item.deleteDisabledReason ? undefined : () => handleDeleteClick(item)}
        />
      {/each}
      {#if onNewCommit}
        <div class="add-commit-row">
          <button
            class="add-commit-btn"
            onclick={onNewCommit}
            disabled={newSessionDisabled}
            title="New commit"
          >
            <Plus size={13} />
            <span>New commit</span>
          </button>
        </div>
      {/if}
    </div>
  {/if}
{/if}

<style>
  /* ── Notes strip ────────────────────────────────────────────────────── */

  .notes-strip {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0 -4px;
  }

  .note-chip {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-radius: 4px;
    background-color: var(--note-bg);
    width: calc((100% - 12px) / 3);
    min-width: 0;
    position: relative;
    transition: background-color 0.15s ease;
  }

  .note-chip.clickable {
    cursor: pointer;
  }

  .note-chip.clickable:hover {
    background-color: var(--note-bg-emphasis);
  }

  .note-chip.pending {
    cursor: default;
  }

  .note-chip.failed {
    cursor: default;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
  }

  .note-chip-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--note-color);
  }

  .note-chip.failed .note-chip-icon {
    color: var(--text-muted);
  }

  .note-chip.pending .note-chip-icon :global(.spinner) {
    color: var(--note-color);
  }

  .note-chip-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .note-chip-title {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 1.4;
  }

  .note-chip-time {
    font-size: var(--size-xs);
    color: var(--text-faint);
    line-height: 1.3;
  }

  .note-chip.pending .note-chip-title {
    color: var(--text-muted);
    font-style: italic;
    font-weight: normal;
  }

  .note-chip.failed .note-chip-title {
    color: var(--text-muted);
    font-style: italic;
    font-weight: normal;
  }

  .note-chip.failed .note-chip-time {
    color: var(--text-muted);
  }

  /* Hover actions on note chips — overlaid on the right */
  .note-chip-actions {
    display: flex;
    align-items: center;
    gap: 1px;
    position: absolute;
    right: 4px;
    top: 50%;
    transform: translateY(-50%);
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.1s;
  }

  .note-chip:hover .note-chip-actions {
    opacity: 1;
    pointer-events: auto;
  }

  .chip-action-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
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

  .chip-action-btn.session-btn:hover {
    color: var(--ui-accent);
    background: var(--bg-hover);
  }

  .chip-action-btn.delete-btn:hover {
    color: var(--ui-danger);
    background: var(--bg-hover);
  }

  /* ── Commit timeline ────────────────────────────────────────────────── */

  .timeline {
    display: flex;
    flex-direction: column;
  }

  .timeline.has-notes {
    margin-top: 10px;
  }

  .no-items {
    margin: 0;
    padding: 8px 0;
    font-size: var(--size-sm);
    color: var(--text-faint);
    font-style: italic;
    text-align: center;
  }

  /* ── Inline add buttons ──────────────────────────────────────────────── */

  .add-note-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 8px 12px;
    border-radius: 4px;
    border: 1px dashed var(--border-subtle);
    background: none;
    color: var(--text-faint);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    width: calc((100% - 12px) / 3);
    min-width: 0;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s;
  }

  .add-note-btn:hover:not(:disabled) {
    color: var(--note-color);
    border-color: var(--note-color);
    background-color: var(--note-bg);
  }

  .add-note-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .add-commit-row {
    padding: 6px 8px;
    margin: 0 -8px;
  }

  .add-commit-btn {
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

  .add-commit-btn:hover:not(:disabled) {
    color: var(--commit-color);
    border-color: var(--commit-color);
    background-color: var(--commit-bg);
  }

  .add-commit-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }
</style>
