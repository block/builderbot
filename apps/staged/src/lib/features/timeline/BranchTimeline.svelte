<!--
  BranchTimeline.svelte - Renders the unified timeline for a branch

  Commits, notes, and reviews are merged by timestamp into a single linear list.
  Active pending items (running sessions, generating notes) appear at the bottom.
  Failed sessions appear in chronological order with completed items.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import type { Snippet } from 'svelte';
  import { slide } from 'svelte/transition';
  import { FileText, GitCommitVertical, FileSearch } from 'lucide-svelte';
  import type { BranchTimeline as BranchTimelineData, HashtagItem } from '../../types';
  import TimelineRow from './TimelineRow.svelte';
  import type { TimelineItemType, TimelineBadge } from './TimelineRow.svelte';
  import { hasHashtagTokens, renderHashtagTokens } from '../sessions/hashtagItems';
  import {
    formatRelativeTime,
    formatRelativeTimeSeconds,
    minuteNow,
  } from '../../shared/relativeTime.svelte';
  import {
    collectRunningSessionIds,
    createLiveSessionHints,
    fallbackHintForPendingType,
    type PendingHintItemType,
  } from './liveSessionHints';
  import { isEmptyFailedReview } from './reviewState';
  import { stripXmlTags } from '../sessions/sessionModalHelpers';

  type PendingItem = {
    key: string;
    type: PendingHintItemType | 'queued-commit' | 'queued-note' | 'queued-review';
    title: string;
    secondaryMeta?: string;
    sessionId?: string;
  };

  interface Props {
    timeline: BranchTimelineData;
    /** Repo base directory — tool call paths within it are shown as relative in hints. */
    repoDir?: string | null;
    /** Placeholder items for notes being created from drag-and-drop. */
    pendingDropNotes?: { key: string; title: string }[];
    /** Placeholder items for newly started sessions before timeline persistence catches up. */
    pendingItems?: PendingItem[];
    /** Session IDs that were just pruned from pendingItems because the real timeline item arrived. */
    prunedSessionIds?: Set<string>;
    /** Existing timeline rows currently being deleted (rendered in-place as deleting). */
    deletingItems?: { type: 'commit' | 'note' | 'review' | 'image'; id: string }[];
    onSessionClick?: (sessionId: string) => void;
    onResumeClick?: (sessionId: string) => void;
    onCommitClick?: (sha: string) => void;
    onNoteClick?: (noteId: string, title: string, content: string, sessionId?: string) => void;
    onReviewClick?: (reviewId: string) => void;
    onImageClick?: (imageId: string) => void;
    onDeleteCommit?: (sha: string, sessionId?: string, opts?: { altKey: boolean }) => void;
    onDeletePendingCommit?: (commitId: string, sessionId?: string) => void;
    onDeleteNote?: (noteId: string, sessionId?: string, opts?: { altKey: boolean }) => void;
    onDeleteReview?: (reviewId: string, sessionId?: string, opts?: { altKey: boolean }) => void;
    onDeleteImage?: (imageId: string, opts?: { altKey: boolean }) => void;
    onStartQueued?: () => void;
    /** Optional per-review breakdown of visible comments vs hold-to-reveal annotations. */
    reviewCommentBreakdown?: Record<
      string,
      {
        comments: number;
        annotations: number;
        warnings?: number;
      }
    >;
    onNewNote?: () => void;
    onNewCommit?: () => void;
    onNewReview?: (e: MouseEvent) => void;
    newSessionDisabled?: boolean;
    /** Whether the timeline is being revalidated in the background. */
    revalidating?: boolean;
    /** Error message from a failed load/revalidation. */
    error?: string | null;
    /** Callback to retry loading the timeline. */
    onRetry?: () => void;
    /** When set, a provisioning row is shown at the start of the timeline. */
    provisioningLabel?: string;
    /** Optional detail text for the provisioning row (e.g. git progress). */
    provisioningDetail?: string | null;
    /** Hashtag items for rendering #type:id badges in timeline titles. */
    hashtagItems?: HashtagItem[];
    footerActions?: Snippet;
  }

  let {
    timeline,
    repoDir,
    pendingDropNotes = [],
    pendingItems = [],
    prunedSessionIds = new Set(),
    deletingItems = [],
    onSessionClick,
    onResumeClick,
    onCommitClick,
    onNoteClick,
    onReviewClick,
    onImageClick,
    onDeleteCommit,
    onDeletePendingCommit,
    onDeleteNote,
    onDeleteReview,
    onDeleteImage,
    onStartQueued,
    reviewCommentBreakdown = {},
    onNewNote,
    onNewCommit,
    onNewReview,
    newSessionDisabled = false,
    revalidating = false,
    error,
    onRetry,
    provisioningLabel,
    hashtagItems = [],
    provisioningDetail,
    footerActions,
  }: Props = $props();

  // ── Suppress transitions when a pending item is replaced by a real one ──
  //
  // When a pending session placeholder is pruned because the real timeline
  // row arrived, the parent passes the pruned session IDs via the
  // prunedSessionIds prop.  Both the pending item removal and the real
  // timeline data arrive in the same synchronous block (inside loadTimeline),
  // so Svelte batches them into a single render cycle.
  //
  // We suppress the slide-in on the new real item and the slide-out on
  // the departing pending item so neither animates — the real row simply
  // replaces the placeholder in place.

  /**
   * Slide transition that skips animation when the given session ID is in the
   * pruned set — used for both the intro on the arriving real row and the outro
   * on the departing pending row so neither side animates during replacement.
   */
  function maybeSlide(node: Element, params: { sessionId?: string }) {
    if (params.sessionId && prunedSessionIds.has(params.sessionId)) {
      return { duration: 0 };
    }
    return slide(node, { duration: 200 });
  }

  const artifactNoun: Record<string, string> = {
    commit: 'commit',
    note: 'note',
    review: 'comments',
  };

  function failedSubtitle(
    completionReason: string | null | undefined,
    kind: 'commit' | 'note' | 'review'
  ): string {
    const noun = artifactNoun[kind];
    switch (completionReason) {
      case 'crashed':
        return `Session crashed — no ${noun} created`;
      case 'app_quit':
        return `Session interrupted — no ${noun} created`;
      case 'interrupted':
        return `Session stopped — no ${noun} created`;
      default:
        return `Session finished — no ${noun} created`;
    }
  }

  let liveSessionHints = $state<Record<string, string>>({});
  const liveSessionHintPoller = createLiveSessionHints(
    (nextHints) => {
      liveSessionHints = nextHints;
    },
    () => repoDir
  );

  // Unified timeline item for display
  type DisplayItem = {
    key: string;
    type: TimelineItemType;
    title: string;
    /** Pre-rendered HTML title with hashtag badges (set when title contains tokens). */
    titleHtml?: string;
    meta?: string;
    secondaryMeta?: string;
    deleting?: boolean;
    timestamp: number;
    /** Position in git's topological order (0 = oldest). Tiebreaker for same-second timestamps. */
    order: number;
    sessionId?: string;
    commitSha?: string;
    commitId?: string;
    noteId?: string;
    noteTitle?: string;
    noteContent?: string;
    reviewId?: string;
    imageId?: string;
    imageFilename?: string;
    badges?: TimelineBadge[];
    /** When set, delete button is shown but disabled with this tooltip. */
    deleteDisabledReason?: string;
    completionReason?: string | null;
  };

  let runningSessionIds = $derived.by(() => collectRunningSessionIds(timeline, pendingItems));

  /** True when there is at least one non-queued active session (running in timeline or pending-but-not-queued). */
  let hasActiveSession = $derived.by(() => {
    for (const commit of timeline.commits) {
      if (commit.sessionStatus === 'running') return true;
    }
    for (const note of timeline.notes) {
      if (note.sessionStatus === 'running') return true;
    }
    for (const review of timeline.reviews) {
      if (!review.isAuto && review.sessionStatus === 'running') return true;
    }
    for (const item of pendingItems) {
      if (item.sessionId && !item.type.startsWith('queued-')) return true;
    }
    return false;
  });

  $effect(() => {
    liveSessionHintPoller.syncRunningSessionIds(runningSessionIds);
  });

  onDestroy(() => {
    liveSessionHintPoller.destroy();
  });

  // Merge commits, notes, and reviews into a single sorted list
  let items = $derived.by(() => {
    const nowMs = minuteNow.now();
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
    const deletingImageIds = new Set(
      deletingItems.filter((item) => item.type === 'image').map((item) => item.id)
    );

    for (const commit of timeline.commits) {
      const isPending = !commit.sha;
      const isRunning = commit.sessionStatus === 'running';
      const isQueued = commit.sessionStatus === 'queued';
      const isFailed = isPending && !isRunning && !isQueued && !!commit.sessionId;
      const isDeleting =
        (!!commit.id && deletingCommitIds.has(commit.id)) ||
        (!!commit.sha && deletingCommitIds.has(commit.sha));
      const liveHint = commit.sessionId ? liveSessionHints[commit.sessionId] : undefined;

      let type: TimelineItemType;
      let secondaryMeta: string | undefined;

      if (isFailed) {
        type = 'failed-commit';
        secondaryMeta = failedSubtitle(commit.completionReason, 'commit');
      } else if (isQueued) {
        type = 'queued-commit';
        secondaryMeta = 'Queued';
      } else if (isPending || isRunning) {
        type = 'pending-commit';
        secondaryMeta = liveHint ?? 'Generating commit';
      } else {
        type = 'commit';
        secondaryMeta = formatRelativeTimeSeconds(commit.timestamp, nowMs);
      }

      all.push({
        key: commit.sha || `pending-${commit.sessionId || commit.timestamp}`,
        type,
        title: stripXmlTags(commit.subject),
        meta: isDeleting ? 'Deleting...' : secondaryMeta,
        secondaryMeta: isDeleting || isRunning ? undefined : commit.shortSha || undefined,
        deleting: isDeleting,
        timestamp: commit.timestamp,
        order: commit.order,
        sessionId: commit.sessionId ?? undefined,
        commitSha: commit.sha || undefined,
        commitId: commit.id ?? undefined,
        deleteDisabledReason: isDeleting ? 'Deleting...' : undefined,
        completionReason: commit.completionReason,
      });
    }

    for (const note of timeline.notes) {
      const isRunning = note.sessionStatus === 'running';
      const isQueued = note.sessionStatus === 'queued';
      const isFailed = !isRunning && !isQueued && !!note.sessionId && !note.content?.trim();
      const isDeleting = deletingNoteIds.has(note.id);
      const liveHint = note.sessionId ? liveSessionHints[note.sessionId] : undefined;

      let type: TimelineItemType;
      let secondaryMeta: string | undefined;

      if (isFailed) {
        type = 'failed-note';
        secondaryMeta = failedSubtitle(note.completionReason, 'note');
      } else if (isQueued) {
        type = 'queued-note';
        secondaryMeta = 'Queued';
      } else if (isRunning) {
        type = 'generating-note';
        secondaryMeta = liveHint ?? 'Generating note';
      } else {
        type = 'note';
        secondaryMeta = formatRelativeTime(note.completedAt ?? note.createdAt, nowMs);
      }

      all.push({
        key: `note-${note.id}`,
        type,
        title: stripXmlTags(note.title),
        secondaryMeta: isDeleting ? 'Deleting...' : secondaryMeta,
        deleting: isDeleting,
        // Use completedAt so completed notes sort by completion time, not queue time
        timestamp: Math.floor((note.completedAt ?? note.createdAt) / 1000),
        order: 0,
        sessionId: note.sessionId ?? undefined,
        noteId: note.id,
        noteTitle: stripXmlTags(note.title),
        noteContent: note.content,
        deleteDisabledReason: isDeleting ? 'Deleting...' : undefined,
        completionReason: note.completionReason,
      });
    }

    for (const review of timeline.reviews) {
      if (review.isAuto) continue;
      const breakdown = reviewCommentBreakdown[review.id];
      const commentCount = breakdown?.comments ?? review.commentCount;
      const annotationCount = breakdown?.annotations ?? 0;
      const totalCount = commentCount + annotationCount;
      const isRunning = review.sessionStatus === 'running';
      const isQueued = review.sessionStatus === 'queued';
      const isFailed = isEmptyFailedReview({
        sessionStatus: review.sessionStatus,
        sessionId: review.sessionId,
        title: review.title,
        totalCount,
      });
      const isDeleting = deletingReviewIds.has(review.id);
      const liveHint = review.sessionId ? liveSessionHints[review.sessionId] : undefined;

      // Build badges: warnings get their own badge, everything else is a comment
      const warningCount = breakdown?.warnings ?? 0;
      const nonWarningCount = commentCount - warningCount;

      const badges: TimelineBadge[] = [];
      if (warningCount > 0) {
        badges.push({ icon: 'warning', count: warningCount });
      }
      if (nonWarningCount > 0) {
        badges.push({ icon: 'comment', count: nonWarningCount });
      }

      let type: TimelineItemType;
      let meta: string | undefined;

      if (isFailed) {
        type = 'failed-review';
        meta = failedSubtitle(review.completionReason, 'review');
      } else if (isQueued) {
        type = 'queued-review';
        meta = 'Queued';
      } else if (isRunning) {
        type = 'generating-review';
        meta = liveHint ?? 'Generating review';
      } else {
        type = 'review';
        meta = formatRelativeTime(review.completedAt ?? review.createdAt, nowMs);
      }

      all.push({
        key: `review-${review.id}`,
        type,
        title: review.title || 'Code Review',
        meta: isDeleting ? 'Deleting...' : meta,
        badges: badges.length > 0 ? badges : undefined,
        deleting: isDeleting,
        // Use completedAt so completed reviews sort by completion time, not queue time
        timestamp: Math.floor((review.completedAt ?? review.createdAt) / 1000),
        order: 0,
        sessionId: review.sessionId ?? undefined,
        reviewId: review.id,
        deleteDisabledReason: isDeleting ? 'Deleting...' : undefined,
        completionReason: review.completionReason,
      });
    }

    for (const image of timeline.images) {
      const isDeleting = deletingImageIds.has(image.id);

      all.push({
        key: `image-${image.id}`,
        type: 'image' as TimelineItemType,
        title: image.filename,
        secondaryMeta: isDeleting ? 'Deleting...' : formatRelativeTime(image.createdAt, nowMs),
        deleting: isDeleting,
        timestamp: Math.floor(image.createdAt / 1000),
        order: 0,
        sessionId: image.sessionId ?? undefined,
        imageId: image.id,
        imageFilename: image.filename,
        deleteDisabledReason: isDeleting ? 'Deleting...' : undefined,
      });
    }

    // Provisioning row appears at the very start of the timeline
    if (provisioningLabel) {
      all.unshift({
        key: 'provisioning',
        type: 'provisioning',
        title: provisioningLabel,
        secondaryMeta: provisioningDetail ?? undefined,
        timestamp: 0, // only one provisioning item exists, so order is irrelevant
        order: 0,
      });
    }

    // Render hashtag badges in titles that contain #type:id tokens
    if (hashtagItems.length > 0) {
      for (const item of all) {
        if (hasHashtagTokens(item.title)) {
          item.titleHtml = renderHashtagTokens(item.title, hashtagItems);
        }
      }
    }

    // Sort by timestamp ascending; pending/generating items at bottom, queued after those
    all.sort((a, b) => {
      const isProvisioning = (item: DisplayItem) => item.type === 'provisioning';
      const isTransient = (item: DisplayItem) =>
        item.type === 'pending-commit' ||
        item.type === 'generating-note' ||
        item.type === 'generating-review';
      const isQueued = (item: DisplayItem) => item.type.startsWith('queued-');

      const aIsProvisioning = isProvisioning(a);
      const bIsProvisioning = isProvisioning(b);
      const aIsTransient = isTransient(a);
      const bIsTransient = isTransient(b);
      const aIsQueued = isQueued(a);
      const bIsQueued = isQueued(b);

      // Provisioning < Completed < Active < Queued
      const aOrder = aIsProvisioning ? -1 : aIsQueued ? 2 : aIsTransient ? 1 : 0;
      const bOrder = bIsProvisioning ? -1 : bIsQueued ? 2 : bIsTransient ? 1 : 0;
      if (aOrder !== bOrder) return aOrder - bOrder;
      if (a.timestamp !== b.timestamp) return a.timestamp - b.timestamp;
      return a.order - b.order;
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

  /** True when the timeline has no content and action buttons should be enlarged. */
  let actionButtonsEnlarged = $derived(
    items.length === 0 && pendingDropNotes.length === 0 && pendingItems.length === 0
  );

  // ── Handlers ──────────────────────────────────────────────────────────

  function handleItemClick(item: DisplayItem) {
    if (item.type === 'commit' && item.commitSha && onCommitClick) {
      onCommitClick(item.commitSha);
    } else if (item.type === 'note' && item.noteId && onNoteClick) {
      onNoteClick(item.noteId, item.noteTitle ?? '', item.noteContent ?? '', item.sessionId);
    } else if (item.type === 'review' && item.reviewId && onReviewClick) {
      onReviewClick(item.reviewId);
    } else if (item.type === 'image' && item.imageId && onImageClick) {
      onImageClick(item.imageId);
    }
  }

  const resumableReasons = new Set(['crashed', 'app_quit', 'interrupted']);

  function isResumable(item: DisplayItem): boolean {
    return (
      !!item.sessionId &&
      !!item.completionReason &&
      resumableReasons.has(item.completionReason) &&
      !item.deleting
    );
  }

  function handleDeleteClick(item: DisplayItem, opts?: { altKey: boolean }) {
    if (item.type === 'commit' && item.commitSha && onDeleteCommit) {
      onDeleteCommit(item.commitSha, item.sessionId, opts);
    } else if (
      (item.type === 'failed-commit' ||
        item.type === 'pending-commit' ||
        item.type === 'queued-commit') &&
      item.commitId &&
      onDeletePendingCommit
    ) {
      onDeletePendingCommit(item.commitId, item.sessionId);
    } else if (
      (item.type === 'note' ||
        item.type === 'failed-note' ||
        item.type === 'generating-note' ||
        item.type === 'queued-note') &&
      item.noteId &&
      onDeleteNote
    ) {
      onDeleteNote(item.noteId, item.sessionId, opts);
    } else if (
      (item.type === 'review' ||
        item.type === 'failed-review' ||
        item.type === 'generating-review' ||
        item.type === 'queued-review') &&
      item.reviewId &&
      onDeleteReview
    ) {
      onDeleteReview(item.reviewId, item.sessionId, opts);
    } else if (item.type === 'image' && item.imageId && onDeleteImage) {
      onDeleteImage(item.imageId, opts);
    }
  }
</script>

{#if items.length === 0 && !onNewNote && !onNewCommit && !onNewReview && pendingDropNotes.length === 0 && pendingItems.length === 0}
  <p class="no-items">No commits or notes yet</p>
{:else}
  <!-- Unified timeline (vertical) -->
  <div class="timeline">
    {#each items as item, index (item.key)}
      <div in:maybeSlide={{ sessionId: item.sessionId }} out:slide={{ duration: 200 }}>
        <TimelineRow
          type={item.type}
          title={item.title}
          titleHtml={item.titleHtml}
          meta={item.meta}
          secondaryMeta={item.secondaryMeta}
          badges={item.badges}
          deleting={item.deleting}
          isLast={index === items.length - 1 &&
            !onNewNote &&
            !onNewCommit &&
            pendingDropNotes.length === 0 &&
            pendingItems.length === 0 &&
            !revalidating &&
            !error}
          sessionId={item.sessionId}
          deleteDisabledReason={item.deleteDisabledReason}
          {onSessionClick}
          onItemClick={() => handleItemClick(item)}
          onDeleteClick={item.deleteDisabledReason
            ? undefined
            : (opts) => handleDeleteClick(item, opts)}
          onStartClick={item.type.startsWith('queued-') && !hasActiveSession
            ? onStartQueued
            : undefined}
          onResumeClick={isResumable(item) && onResumeClick && item.sessionId
            ? () => onResumeClick!(item.sessionId!)
            : undefined}
        />
      </div>
    {/each}
    {#each pendingDropNotes as drop, index (drop.key)}
      <div transition:slide={{ duration: 200 }}>
        <TimelineRow
          type="generating-note"
          title={drop.title}
          secondaryMeta="adding..."
          isLast={index === pendingDropNotes.length - 1 &&
            pendingItems.length === 0 &&
            !revalidating &&
            !error &&
            !onNewNote &&
            !onNewCommit}
        />
      </div>
    {/each}
    {#each pendingItems as item, index (item.key)}
      <div in:slide={{ duration: 200 }} out:maybeSlide={{ sessionId: item.sessionId }}>
        <TimelineRow
          type={item.type}
          title={item.title}
          secondaryMeta={item.sessionId
            ? (liveSessionHints[item.sessionId] ??
              item.secondaryMeta ??
              fallbackHintForPendingType(item.type))
            : item.secondaryMeta}
          isLast={index === pendingItems.length - 1 &&
            !revalidating &&
            !error &&
            !onNewNote &&
            !onNewCommit}
        />
      </div>
    {/each}
    {#if revalidating}
      <div transition:slide={{ duration: 200 }}>
        <TimelineRow
          type="revalidating"
          title="Looking for changes..."
          isLast={!error && !onNewNote && !onNewCommit}
        />
      </div>
    {/if}
    {#if error && !revalidating}
      <div transition:slide={{ duration: 200 }}>
        <TimelineRow
          type="load-error"
          title="Failed to load commits"
          secondaryMeta={error}
          isLast={true}
          onRetryClick={onRetry}
        />
      </div>
    {/if}
    {#if onNewNote || onNewCommit || onNewReview || footerActions}
      <div class="footer-row" class:footer-row-enlarged={actionButtonsEnlarged}>
        <div class="footer-left-actions" class:footer-left-actions-enlarged={actionButtonsEnlarged}>
          {#if onNewNote}
            <button
              class="add-item-btn note-btn"
              class:add-item-btn-enlarged={actionButtonsEnlarged}
              onclick={onNewNote}
              disabled={newSessionDisabled}
              title="New note"
            >
              <FileText size={18} />
              <span>New note</span>
            </button>
          {/if}
          {#if onNewCommit}
            <button
              class="add-item-btn commit-btn"
              class:add-item-btn-enlarged={actionButtonsEnlarged}
              onclick={onNewCommit}
              disabled={newSessionDisabled}
              title="New commit"
            >
              <GitCommitVertical size={18} />
              <span>New commit</span>
            </button>
          {/if}
          {#if onNewReview}
            <button
              class="add-item-btn review-btn"
              class:add-item-btn-enlarged={actionButtonsEnlarged}
              onclick={(e) => onNewReview?.(e)}
              disabled={newSessionDisabled}
              title="New code review"
            >
              <FileSearch size={18} />
              <span>New code review</span>
            </button>
          {/if}
        </div>
        {#if footerActions && !actionButtonsEnlarged}
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
    transition:
      gap 0.3s ease,
      padding 0.3s ease,
      margin 0.3s ease;
  }

  .footer-row-enlarged {
    gap: 10px;
    padding: 4px 0;
    margin: 0;
  }

  .footer-left-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    transition: gap 0.3s ease;
  }

  .footer-left-actions-enlarged {
    gap: 10px;
    flex: 1;
  }

  .add-item-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 4px 10px;
    border-radius: 6px;
    border: 1px dashed var(--border-subtle);
    background: transparent;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s,
      padding 0.3s ease,
      border-radius 0.3s ease,
      gap 0.3s ease,
      font-size 0.3s ease,
      flex 0.3s ease,
      border-style 0.3s ease;
  }

  .add-item-btn :global(svg) {
    transition:
      width 0.3s ease,
      height 0.3s ease;
    width: 13px;
    height: 13px;
  }

  .add-item-btn-enlarged {
    flex: 1;
    justify-content: center;
    gap: 8px;
    padding: 10px 6px;
    border-radius: 8px;
    border-color: transparent;
    border-style: solid;
    background: var(--bg-elevated);
    font-size: var(--size-sm);
  }

  .add-item-btn-enlarged :global(svg) {
    width: 18px;
    height: 18px;
  }

  .add-item-btn-enlarged.note-btn:hover:not(:disabled) {
    color: var(--note-color);
    background-color: var(--note-bg);
  }

  .add-item-btn-enlarged.commit-btn:hover:not(:disabled) {
    color: var(--commit-color);
    background-color: var(--commit-bg);
  }

  .add-item-btn-enlarged.review-btn:hover:not(:disabled) {
    color: var(--review-color);
    background-color: var(--review-bg);
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
