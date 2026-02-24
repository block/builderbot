<!--
  BranchTimeline.svelte - Renders the unified timeline for a branch

  Commits, notes, and reviews are merged by timestamp into a single linear list.
  Active pending items (running sessions, generating notes) appear at the bottom.
  Failed sessions appear in chronological order with completed items.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import type { Snippet } from 'svelte';
  import { FileText, GitCommitVertical, FileSearch } from 'lucide-svelte';
  import * as commands from '../../commands';
  import type { BranchTimeline as BranchTimelineData, SessionMessage } from '../../types';
  import TimelineRow from './TimelineRow.svelte';
  import type { TimelineItemType } from './TimelineRow.svelte';

  interface Props {
    timeline: BranchTimelineData;
    /** Placeholder items for notes being created from drag-and-drop. */
    pendingDropNotes?: { key: string; title: string }[];
    /** Placeholder items for newly started sessions before timeline persistence catches up. */
    pendingItems?: {
      key: string;
      type: 'pending-commit' | 'generating-note' | 'generating-review';
      title: string;
      secondaryMeta?: string;
      sessionId?: string;
    }[];
    /** Existing timeline rows currently being deleted (rendered in-place as deleting). */
    deletingItems?: { type: 'commit' | 'note' | 'review'; id: string }[];
    onSessionClick?: (sessionId: string) => void;
    onCommitClick?: (sha: string) => void;
    onNoteClick?: (noteId: string, title: string, content: string) => void;
    onReviewClick?: (reviewId: string) => void;
    onDeleteCommit?: (sha: string, sessionId?: string) => void;
    onDeletePendingCommit?: (commitId: string, sessionId?: string) => void;
    onDeleteNote?: (noteId: string, sessionId?: string) => void;
    onDeleteReview?: (reviewId: string, sessionId?: string) => void;
    onNewNote?: () => void;
    onNewCommit?: () => void;
    onNewReview?: () => void;
    newSessionDisabled?: boolean;
    footerActions?: Snippet;
  }

  let {
    timeline,
    pendingDropNotes = [],
    pendingItems = [],
    deletingItems = [],
    onSessionClick,
    onCommitClick,
    onNoteClick,
    onReviewClick,
    onDeleteCommit,
    onDeletePendingCommit,
    onDeleteNote,
    onDeleteReview,
    onNewNote,
    onNewCommit,
    onNewReview,
    newSessionDisabled = false,
    footerActions,
  }: Props = $props();

  // Disable creating new branch sessions while one is actively generating.
  let hasRunningSessionGeneration = $derived(
    timeline.commits.some((commit) => commit.sessionStatus === 'running') ||
      timeline.notes.some((note) => note.sessionStatus === 'running') ||
      timeline.reviews.some((review) => review.sessionStatus === 'running')
  );
  let disableNewSessionActions = $derived(newSessionDisabled || hasRunningSessionGeneration);

  const HINT_POLL_INTERVAL_MS = 750;
  const MAX_HINT_LENGTH = 72;
  const MAX_HINT_MESSAGES = 40;

  type HintTracker = {
    lastMessageId: number | null;
    messages: SessionMessage[];
  };

  const hintTrackers = new Map<string, HintTracker>();
  let liveSessionHints = $state<Record<string, string>>({});
  let hintPollTimer: ReturnType<typeof setInterval> | null = null;
  let hintPollInFlight = false;

  // Unified timeline item for display
  type DisplayItem = {
    key: string;
    type: TimelineItemType;
    title: string;
    meta?: string;
    secondaryMeta?: string;
    deleting?: boolean;
    timestamp: number;
    sessionId?: string;
    commitSha?: string;
    commitId?: string;
    noteId?: string;
    noteTitle?: string;
    noteContent?: string;
    reviewId?: string;
    /** When set, delete button is shown but disabled with this tooltip. */
    deleteDisabledReason?: string;
  };

  /** Strip XML-tagged context blocks (action, branch-history) from display text. */
  function stripXmlTags(text: string): string {
    return text.replace(/<(action|branch-history)>[\s\S]*?<\/\1>/g, '').trim();
  }

  function fallbackHintForPendingType(type: TimelineItemType): string {
    if (type === 'pending-commit') return 'Generating commit';
    if (type === 'generating-review') return 'Generating review';
    return 'Generating note';
  }

  function normalizeHintText(text: string): string | undefined {
    const cleaned = stripXmlTags(text)
      .replace(/```[\s\S]*?```/g, ' ')
      .replace(/<\/?[^>]+>/g, ' ')
      .replace(/\s+/g, ' ')
      .trim();
    if (!cleaned) return undefined;
    if (cleaned.length <= MAX_HINT_LENGTH) return cleaned;
    return `${cleaned.slice(0, MAX_HINT_LENGTH - 1).trimEnd()}…`;
  }

  function withTrailingEllipsis(text: string): string {
    if (/[.!?…]$/.test(text)) {
      if (text.endsWith('.')) {
        return `${text.slice(0, -1)}…`;
      }
      return text;
    }
    return `${text}…`;
  }

  function formatToolCallHint(content: string): string | undefined {
    const normalized = normalizeHintText(content);
    if (!normalized) return undefined;

    const withProgressVerb = normalized
      .replace(/^Run\s+/i, 'Running ')
      .replace(/^Read\s+/i, 'Reading ')
      .replace(/^Write\s+/i, 'Writing ')
      .replace(/^Open\s+/i, 'Opening ')
      .replace(/^Search\s+/i, 'Searching ')
      .replace(/^List\s+/i, 'Listing ')
      .replace(/^Create\s+/i, 'Creating ')
      .replace(/^Update\s+/i, 'Updating ')
      .replace(/^Delete\s+/i, 'Deleting ')
      .replace(/^Fetch\s+/i, 'Fetching ')
      .replace(/^Draft\s+/i, 'Drafting ');

    return normalizeHintText(withTrailingEllipsis(withProgressVerb));
  }

  function formatAssistantHint(content: string): string | undefined {
    const lines = stripXmlTags(content)
      .replace(/```[\s\S]*?```/g, ' ')
      .split(/\r?\n/)
      .map((line) =>
        line
          .trim()
          .replace(/^[-*]\s+/, '')
          .replace(/^\d+\.\s+/, '')
          .replace(/^#+\s+/, '')
      )
      .filter(Boolean);

    for (let i = lines.length - 1; i >= 0; i--) {
      const normalized = normalizeHintText(lines[i]);
      if (normalized) {
        return normalized;
      }
    }

    return undefined;
  }

  function deriveHint(messages: SessionMessage[]): string | undefined {
    let latestToolHint: { id: number; text: string } | null = null;
    let latestAssistantHint: { id: number; text: string } | null = null;

    for (let i = messages.length - 1; i >= 0; i--) {
      const message = messages[i];
      if (!latestToolHint && message.role === 'tool_call') {
        const hint = formatToolCallHint(message.content);
        if (hint) {
          latestToolHint = { id: message.id, text: hint };
        }
      }
      if (!latestAssistantHint && message.role === 'assistant') {
        const hint = formatAssistantHint(message.content);
        if (hint) {
          latestAssistantHint = { id: message.id, text: hint };
        }
      }
      if (latestToolHint && latestAssistantHint) {
        break;
      }
    }

    if (latestToolHint && (!latestAssistantHint || latestToolHint.id >= latestAssistantHint.id)) {
      return latestToolHint.text;
    }
    if (latestAssistantHint) {
      return latestAssistantHint.text;
    }
    if (latestToolHint) {
      return latestToolHint.text;
    }
    return undefined;
  }

  function setHint(sessionId: string, hint: string) {
    if (liveSessionHints[sessionId] === hint) return;
    liveSessionHints = { ...liveSessionHints, [sessionId]: hint };
  }

  function clearHint(sessionId: string) {
    if (!(sessionId in liveSessionHints)) return;
    const next = { ...liveSessionHints };
    delete next[sessionId];
    liveSessionHints = next;
  }

  function mergeHintMessages(
    previous: SessionMessage[],
    updated: SessionMessage[],
    hadPrevious: boolean
  ): SessionMessage[] {
    if (updated.length === 0) return previous;
    const merged = hadPrevious ? [...previous.slice(0, -1), ...updated] : updated;
    return merged.slice(-MAX_HINT_MESSAGES);
  }

  async function refreshHint(sessionId: string, tracker: HintTracker) {
    try {
      const session = await commands.getSession(sessionId);
      if (!session || session.status !== 'running') {
        hintTrackers.delete(sessionId);
        clearHint(sessionId);
        return;
      }

      const updatedMessages =
        tracker.lastMessageId === null
          ? await commands.getSessionMessages(sessionId)
          : await commands.getSessionMessagesSince(sessionId, tracker.lastMessageId);

      if (!hintTrackers.has(sessionId)) return;

      if (updatedMessages.length > 0) {
        tracker.messages = mergeHintMessages(
          tracker.messages,
          updatedMessages,
          tracker.lastMessageId !== null
        );
        const latestMessage = tracker.messages[tracker.messages.length - 1];
        tracker.lastMessageId = latestMessage?.id ?? tracker.lastMessageId;
      }

      const nextHint = deriveHint(tracker.messages);
      if (nextHint) {
        setHint(sessionId, nextHint);
      } else {
        clearHint(sessionId);
      }
    } catch {
      // Fail-safe: keep existing static fallback labels when hint polling fails.
    }
  }

  async function pollHints() {
    if (hintPollInFlight || hintTrackers.size === 0) return;
    hintPollInFlight = true;

    try {
      const entries = Array.from(hintTrackers.entries());
      await Promise.all(entries.map(([sessionId, tracker]) => refreshHint(sessionId, tracker)));
    } finally {
      hintPollInFlight = false;
      if (hintTrackers.size === 0) {
        stopHintPolling();
      }
    }
  }

  function startHintPolling() {
    if (hintPollTimer) return;
    hintPollTimer = setInterval(() => {
      void pollHints();
    }, HINT_POLL_INTERVAL_MS);
  }

  function stopHintPolling() {
    if (hintPollTimer) {
      clearInterval(hintPollTimer);
      hintPollTimer = null;
    }
    hintPollInFlight = false;
  }

  let runningSessionIds = $derived.by(() => {
    const ids = new Set<string>();

    for (const commit of timeline.commits) {
      if (commit.sessionStatus === 'running' && commit.sessionId) {
        ids.add(commit.sessionId);
      }
    }
    for (const note of timeline.notes) {
      if (note.sessionStatus === 'running' && note.sessionId) {
        ids.add(note.sessionId);
      }
    }
    for (const review of timeline.reviews) {
      if (review.sessionStatus === 'running' && review.sessionId) {
        ids.add(review.sessionId);
      }
    }
    for (const item of pendingItems) {
      if (!item.sessionId) continue;
      if (
        item.type === 'pending-commit' ||
        item.type === 'generating-note' ||
        item.type === 'generating-review'
      ) {
        ids.add(item.sessionId);
      }
    }

    return Array.from(ids);
  });

  $effect(() => {
    const activeIds = new Set(runningSessionIds);

    for (const sessionId of activeIds) {
      if (!hintTrackers.has(sessionId)) {
        hintTrackers.set(sessionId, {
          lastMessageId: null,
          messages: [],
        });
      }
    }

    for (const sessionId of Array.from(hintTrackers.keys())) {
      if (!activeIds.has(sessionId)) {
        hintTrackers.delete(sessionId);
        clearHint(sessionId);
      }
    }

    if (hintTrackers.size > 0) {
      startHintPolling();
      void pollHints();
    } else {
      stopHintPolling();
    }
  });

  onDestroy(() => {
    stopHintPolling();
    hintTrackers.clear();
  });

  // Merge commits, notes, and reviews into a single sorted list
  let items = $derived.by(() => {
    const all: DisplayItem[] = [];
    const deletingCommitIds = new Set(
      deletingItems.filter((item) => item.type === 'commit').map((item) => item.id)
    );
    const deletingNoteIds = new Set(
      deletingItems.filter((item) => item.type === 'note').map((item) => item.id)
    );
    const deletingReviewIds = new Set(
      deletingItems.filter((item) => item.type === 'review').map((item) => item.id)
    );

    for (const commit of timeline.commits) {
      const isPending = !commit.sha;
      const isRunning = commit.sessionStatus === 'running';
      const isFailed = isPending && !isRunning && !!commit.sessionId;
      const isDeleting = !!commit.id && deletingCommitIds.has(commit.id);
      const liveHint = commit.sessionId ? liveSessionHints[commit.sessionId] : undefined;

      let type: TimelineItemType;
      let secondaryMeta: string | undefined;

      if (isFailed) {
        type = 'failed-commit';
        secondaryMeta = 'Session finished — no commit created';
      } else if (isPending || isRunning) {
        type = 'pending-commit';
        secondaryMeta = liveHint ?? 'Generating commit';
      } else {
        type = 'commit';
        secondaryMeta = formatRelativeTime(commit.timestamp);
      }

      all.push({
        key: commit.sha || `pending-${commit.sessionId || commit.timestamp}`,
        type,
        title: stripXmlTags(commit.subject),
        meta: isDeleting ? 'Deleting...' : secondaryMeta,
        secondaryMeta: commit.shortSha || undefined,
        deleting: isDeleting,
        timestamp: commit.timestamp,
        sessionId: commit.sessionId ?? undefined,
        commitSha: commit.sha || undefined,
        commitId: commit.id ?? undefined,
        deleteDisabledReason: isDeleting ? 'Deleting...' : undefined,
      });
    }

    for (const note of timeline.notes) {
      const isRunning = note.sessionStatus === 'running';
      const isFailed = !isRunning && !!note.sessionId && !note.content?.trim();
      const isDeleting = deletingNoteIds.has(note.id);
      const liveHint = note.sessionId ? liveSessionHints[note.sessionId] : undefined;

      let type: TimelineItemType;
      let secondaryMeta: string | undefined;

      if (isFailed) {
        type = 'failed-note';
        secondaryMeta = 'Session finished — no note created';
      } else if (isRunning) {
        type = 'generating-note';
        secondaryMeta = liveHint ?? 'Generating note';
      } else {
        type = 'note';
        secondaryMeta = formatRelativeTimeMs(note.createdAt);
      }

      all.push({
        key: `note-${note.id}`,
        type,
        title: stripXmlTags(note.title),
        secondaryMeta: isDeleting ? 'Deleting...' : secondaryMeta,
        deleting: isDeleting,
        // Note timestamps are in milliseconds, convert to seconds for sorting
        timestamp: Math.floor(note.createdAt / 1000),
        sessionId: note.sessionId ?? undefined,
        noteId: note.id,
        noteTitle: stripXmlTags(note.title),
        noteContent: note.content,
        deleteDisabledReason: isDeleting ? 'Deleting...' : undefined,
      });
    }

    for (const review of timeline.reviews) {
      const isRunning = review.sessionStatus === 'running';
      const isFailed = !isRunning && !!review.sessionId && review.commentCount === 0;
      const isDeleting = deletingReviewIds.has(review.id);
      const liveHint = review.sessionId ? liveSessionHints[review.sessionId] : undefined;

      let type: TimelineItemType;
      let secondaryMeta: string | undefined;

      if (isFailed) {
        type = 'failed-review';
        secondaryMeta = 'Session finished — no comments created';
      } else if (isRunning) {
        type = 'generating-review';
        secondaryMeta = liveHint ?? 'Generating review';
      } else {
        type = 'review';
        secondaryMeta = formatRelativeTimeMs(review.createdAt);
      }

      all.push({
        key: `review-${review.id}`,
        type,
        title: `Code Review`,
        meta: review.commentCount > 0 ? `${review.commentCount} comments` : undefined,
        secondaryMeta: isDeleting ? 'Deleting...' : secondaryMeta,
        deleting: isDeleting,
        timestamp: Math.floor(review.createdAt / 1000),
        sessionId: review.sessionId ?? undefined,
        reviewId: review.id,
        deleteDisabledReason: isDeleting ? 'Deleting...' : undefined,
      });
    }

    // Sort by timestamp ascending; pending/generating items at bottom
    all.sort((a, b) => {
      const aIsTransient =
        a.type === 'pending-commit' ||
        a.type === 'generating-note' ||
        a.type === 'generating-review';
      const bIsTransient =
        b.type === 'pending-commit' ||
        b.type === 'generating-note' ||
        b.type === 'generating-review';
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
    } else if (item.type === 'review' && item.reviewId && onReviewClick) {
      onReviewClick(item.reviewId);
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
    } else if (
      (item.type === 'review' ||
        item.type === 'failed-review' ||
        item.type === 'generating-review') &&
      item.reviewId &&
      onDeleteReview
    ) {
      onDeleteReview(item.reviewId, item.sessionId);
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

{#if items.length === 0 && !onNewNote && !onNewCommit && !onNewReview && pendingDropNotes.length === 0 && pendingItems.length === 0}
  <p class="no-items">No commits or notes yet</p>
{:else if items.length === 0 && pendingDropNotes.length === 0 && pendingItems.length === 0}
  <!-- Empty state: large action buttons -->
  <div class="empty-state">
    {#if onNewNote}
      <button
        class="empty-action-btn note-action"
        onclick={onNewNote}
        disabled={disableNewSessionActions}
      >
        <FileText size={18} />
        <span>New note</span>
      </button>
    {/if}
    {#if onNewCommit}
      <button
        class="empty-action-btn commit-action"
        onclick={onNewCommit}
        disabled={disableNewSessionActions}
      >
        <GitCommitVertical size={18} />
        <span>New commit</span>
      </button>
    {/if}
    {#if onNewReview}
      <button
        class="empty-action-btn review-action"
        onclick={onNewReview}
        disabled={disableNewSessionActions}
      >
        <FileSearch size={18} />
        <span>New code review</span>
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
        deleting={item.deleting}
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
        isLast={index === pendingDropNotes.length - 1 &&
          pendingItems.length === 0 &&
          !onNewNote &&
          !onNewCommit}
      />
    {/each}
    {#each pendingItems as item, index (item.key)}
      <TimelineRow
        type={item.type}
        title={item.title}
        secondaryMeta={item.sessionId
          ? (liveSessionHints[item.sessionId] ??
            item.secondaryMeta ??
            fallbackHintForPendingType(item.type))
          : item.secondaryMeta}
        sessionId={item.sessionId}
        isLast={index === pendingItems.length - 1 && !onNewNote && !onNewCommit}
      />
    {/each}
    {#if onNewNote || onNewCommit || onNewReview || footerActions}
      <div class="footer-row">
        <div class="footer-left-actions">
          {#if onNewNote}
            <button
              class="add-item-btn note-btn"
              onclick={onNewNote}
              disabled={disableNewSessionActions}
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
              disabled={disableNewSessionActions}
              title="New commit"
            >
              <GitCommitVertical size={13} />
              <span>New commit</span>
            </button>
          {/if}
          {#if onNewReview}
            <button
              class="add-item-btn review-btn"
              onclick={onNewReview}
              disabled={disableNewSessionActions}
              title="New code review"
            >
              <FileSearch size={13} />
              <span>New code review</span>
            </button>
          {/if}
        </div>
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
    color: var(--text-muted);
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

  /* Colored icons only in passive state */
  .empty-action-btn.note-action :global(svg) {
    color: var(--note-color);
  }

  .empty-action-btn.commit-action :global(svg) {
    color: var(--commit-color);
  }

  .empty-action-btn.review-action :global(svg) {
    color: var(--review-color);
  }

  .empty-action-btn.note-action:hover:not(:disabled) {
    color: var(--note-color);
    background-color: var(--note-bg);
  }

  .empty-action-btn.commit-action:hover:not(:disabled) {
    color: var(--commit-color);
    background-color: var(--commit-bg);
  }

  .empty-action-btn.review-action:hover:not(:disabled) {
    color: var(--review-color);
    background-color: var(--review-bg);
  }

  .empty-action-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  /* ── Footer row with inline add buttons ─────────────────────────────── */

  .footer-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    padding: 6px 8px;
    margin: 0 -8px;
    position: relative;
    z-index: 1;
  }

  .footer-left-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .add-item-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 4px 10px;
    border-radius: 6px;
    border: 1px dashed var(--border-subtle);
    background: none;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s;
  }

  /* Colored icons only in passive state */
  .add-item-btn.note-btn :global(svg) {
    color: var(--note-color);
  }

  .add-item-btn.commit-btn :global(svg) {
    color: var(--commit-color);
  }

  .add-item-btn.review-btn :global(svg) {
    color: var(--review-color);
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

  .add-item-btn.review-btn:hover:not(:disabled) {
    color: var(--review-color);
    border-color: var(--review-color);
    background-color: var(--review-bg);
  }

  .add-item-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }
</style>
