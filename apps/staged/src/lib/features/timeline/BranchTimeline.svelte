<!--
  BranchTimeline.svelte - Renders the unified timeline for a branch

  Commits, notes, and reviews are merged by timestamp into a single linear list.
  Active pending items (running sessions, generating notes) appear near the bottom.
  Bottom git status rows appear below active and queued work.
  Failed sessions appear in chronological order with completed items.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import type { Snippet } from 'svelte';
  import { slide } from 'svelte/transition';
  import FileText from '@lucide/svelte/icons/file-text';
  import GitCommitVertical from '@lucide/svelte/icons/git-commit-vertical';
  import FileSearch from '@lucide/svelte/icons/file-search';
  import Plus from '@lucide/svelte/icons/plus';
  import { isResumableReason } from '../../types';
  import type {
    BranchGitState,
    BranchTimeline as BranchTimelineData,
    HashtagItem,
  } from '../../types';
  import type { NoteClickInfo } from '../sessions/noteFreshness';
  import TimelineRow from './TimelineRow.svelte';
  import TimelineContextMenu, {
    type TimelineContextMenuAction,
  } from './TimelineContextMenu.svelte';
  import { Button } from '$lib/components/ui/button';
  import type { TimelineItemType, TimelineBadge } from './TimelineRow.svelte';
  import { escapeHtml, hasHashtagTokens, renderHashtagTokens } from '../sessions/hashtagItems';
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
  import { failedArtifactSubtitle } from './sessionFailureCopy';
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
    onNoteClick?: (note: NoteClickInfo) => void;
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
    onPullOrigin?: () => void;
    onPushOrigin?: () => void;
    onRebaseBranch?: () => void;
    onForcePush?: () => void;
    onResetToOrigin?: () => void;
    onOpenForcePushSession?: () => void;
    forcePushingOrigin?: boolean;
    onOpenPushSession?: () => void;
    rebaseBranchDisabledReason?: string | null;
    onViewWorktreeDiff?: () => void;
    onCommitWorktreeChanges?: () => void;
    onDiscardWorktreeChanges?: () => void;
    onNewSessionReferring?: (hashtagRef: string) => void;
    newSessionDisabled?: boolean;
    pullingOrigin?: boolean;
    pushingOrigin?: boolean;
    resettingToOrigin?: boolean;
    discardingWorktreeChanges?: boolean;
    /** Error message from a failed load/revalidation. */
    error?: string | null;
    /** When set, git-mutating row actions are disabled with this reason. */
    gitActionDisabledReason?: string | null;
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
    onPullOrigin,
    onPushOrigin,
    onRebaseBranch,
    onForcePush,
    onResetToOrigin,
    onOpenForcePushSession,
    forcePushingOrigin = false,
    onOpenPushSession,
    rebaseBranchDisabledReason,
    onViewWorktreeDiff,
    onCommitWorktreeChanges,
    onDiscardWorktreeChanges,
    onNewSessionReferring,
    newSessionDisabled = false,
    pullingOrigin = false,
    pushingOrigin = false,
    resettingToOrigin = false,
    discardingWorktreeChanges = false,
    error,
    gitActionDisabledReason,
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
    tertiaryMeta?: string;
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
    noteUpdatedAt?: number;
    reviewId?: string;
    imageId?: string;
    imageFilename?: string;
    badges?: TimelineBadge[];
    onPull?: () => void;
    pullDisabledReason?: string;
    onPush?: () => void;
    pushDisabledReason?: string;
    onRebase?: () => void;
    rebaseDisabledReason?: string;
    onForcePush?: () => void;
    forcePushDisabledReason?: string;
    forcePushing?: boolean;
    onResetToOrigin?: () => void;
    resetToOriginDisabledReason?: string;
    resettingToOrigin?: boolean;
    pushing?: boolean;
    onViewDiff?: () => void;
    onCommitChanges?: () => void;
    commitChangesDisabledReason?: string;
    onDiscardChanges?: () => void;
    discardChangesDisabledReason?: string;
    /** When set, delete button is shown but disabled with this tooltip. */
    deleteDisabledReason?: string;
    completionReason?: string | null;
    /** Hashtag reference token for context menu (e.g. "#commit:abc123"). */
    hashtagRef?: string;
    showConnector?: boolean;
    placement?: 'git-footer';
  };

  type CommitAnchor = {
    timestamp: number;
    order: number;
    shortSha?: string;
  };

  type TimelinePlacement = {
    timestamp: number;
    order: number;
    anchor?: CommitAnchor;
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

  let hasActiveCommitSession = $derived.by(() => {
    for (const commit of timeline.commits) {
      if (commit.sessionStatus === 'running') return true;
    }
    for (const item of pendingItems) {
      if (item.type === 'pending-commit' && item.sessionId) return true;
    }
    return false;
  });

  $effect(() => {
    liveSessionHintPoller.syncRunningSessionIds(runningSessionIds);
  });

  onDestroy(() => {
    liveSessionHintPoller.destroy();
  });

  function plural(count: number, noun: string): string {
    return `${count} ${noun}${count === 1 ? '' : 's'}`;
  }

  function worktreeSummary(state: BranchGitState): string {
    const parts: string[] = [];
    if (state.worktree.conflicted > 0) parts.push(plural(state.worktree.conflicted, 'conflict'));
    if (state.worktree.modified > 0) parts.push(`${state.worktree.modified} modified`);
    if (state.worktree.added > 0) parts.push(`${state.worktree.added} added`);
    if (state.worktree.deleted > 0) parts.push(`${state.worktree.deleted} deleted`);
    if (state.worktree.untracked > 0) parts.push(`${state.worktree.untracked} untracked`);
    return parts.join(', ');
  }

  function withDetail(title: string, detail: string): string {
    return detail ? `${title}: ${detail}` : title;
  }

  function pullDisabledReason(state: BranchGitState): string | undefined {
    if (pullingOrigin) return 'Pulling...';
    if (state.detachedHead) return 'Detached HEAD';
    if (!state.expectedBranchMatches) {
      return state.currentBranch ? `Checked out ${state.currentBranch}` : 'Wrong branch';
    }
    if (state.worktree.dirty) return 'Clean worktree required';
    if (state.upstream.relation !== 'originAhead') return 'Not fast-forwardable';
    return undefined;
  }

  function timelinePlacementAfter(
    commitAnchors: Map<string, CommitAnchor>,
    sha: string | null | undefined,
    fallbackTimestamp: number,
    fallbackOrder: number,
    offset = 0.25
  ): TimelinePlacement {
    const anchor = sha ? commitAnchors.get(sha) : undefined;
    if (!anchor) {
      return { timestamp: fallbackTimestamp, order: fallbackOrder };
    }
    return {
      timestamp: anchor.timestamp,
      order: anchor.order + offset,
      anchor,
    };
  }

  function gitStateRows(
    state: BranchGitState,
    commitAnchors: Map<string, CommitAnchor>
  ): DisplayItem[] {
    const rows: DisplayItem[] = [];
    const topTimestamp = 0;
    const bottomTimestamp = Number.MAX_SAFE_INTEGER - 1000;
    const commitChangesDisabledReason = gitActionDisabledReason
      ? gitActionDisabledReason
      : newSessionDisabled
        ? 'Session in progress'
        : undefined;
    const discardChangesDisabledReason = discardingWorktreeChanges
      ? 'Discarding...'
      : gitActionDisabledReason
        ? gitActionDisabledReason
        : hasActiveSession
          ? 'Session in progress'
          : undefined;

    switch (state.upstream.relation) {
      case 'missing':
        break;
      case 'localAhead':
        {
          const placement = timelinePlacementAfter(
            commitAnchors,
            state.upstream.sha,
            topTimestamp,
            2
          );
          const summary = `is ${plural(state.upstream.ahead, 'commit')} behind`;
          const disabledReason = pushingOrigin
            ? undefined // button is clickable during push (opens session)
            : (rebaseBranchDisabledReason ?? undefined);
          rows.push({
            key: 'git-local-ahead',
            type: 'git-push',
            title: `origin ${summary}`,
            titleHtml: `<span class="git-ref-badge">origin</span> ${escapeHtml(summary)}`,
            timestamp: placement.timestamp,
            order: placement.order,
            onPush: pushingOrigin ? onOpenPushSession : disabledReason ? undefined : onPushOrigin,
            pushDisabledReason: disabledReason,
            pushing: pushingOrigin,
          });
        }
        break;
      case 'originAhead': {
        const disabledReason = pullDisabledReason(state);
        rows.push({
          key: 'git-origin-ahead',
          type: 'git-pull',
          title: `Origin has ${plural(state.upstream.behind, 'new commit')}`,
          timestamp: bottomTimestamp,
          order: 1,
          placement: 'git-footer',
          onPull: disabledReason ? undefined : onPullOrigin,
          pullDisabledReason: disabledReason,
        });
        break;
      }
      case 'diverged': {
        const placement = timelinePlacementAfter(
          commitAnchors,
          state.upstream.mergeBaseSha,
          topTimestamp,
          2
        );
        const behindCount = state.upstream.behind;
        const baseRefShort = state.base.ref.replace(/^origin\//, '') || state.base.ref;
        const upstreamBehindBase = state.upstream.behindBase;
        const baseSummary =
          upstreamBehindBase > 0
            ? ` and is ${plural(upstreamBehindBase, 'commit')} behind ${baseRefShort}`
            : '';
        const divergedTitle = `origin diverges here and has ${plural(behindCount, 'more commit')}${baseSummary}`;
        const divergedTitleHtml = `<span class="git-ref-badge">origin</span> diverges here and has ${escapeHtml(plural(behindCount, 'more commit'))}${escapeHtml(baseSummary)}`;
        const resetToOriginReason = resettingToOrigin
          ? 'Resetting...'
          : forcePushingOrigin
            ? 'Push in progress'
            : onResetToOrigin
              ? (rebaseBranchDisabledReason ?? undefined)
              : undefined;
        rows.push({
          key: 'git-diverged',
          type: 'git-merge-warning',
          title: divergedTitle,
          titleHtml: divergedTitleHtml,
          timestamp: placement.timestamp,
          order: placement.order,
          onForcePush: forcePushingOrigin
            ? onOpenForcePushSession
            : rebaseBranchDisabledReason
              ? undefined
              : onForcePush,
          forcePushDisabledReason: forcePushingOrigin
            ? undefined
            : onForcePush
              ? (rebaseBranchDisabledReason ?? undefined)
              : undefined,
          forcePushing: forcePushingOrigin,
          onResetToOrigin: resetToOriginReason ? undefined : onResetToOrigin,
          resetToOriginDisabledReason: resetToOriginReason,
          resettingToOrigin,
        });
        break;
      }
    }

    if (state.worktree.conflicted > 0 && !hasActiveCommitSession) {
      rows.push({
        key: 'git-conflicted',
        type: 'git-warning',
        title: withDetail('Merge conflicts in worktree', worktreeSummary(state)),
        timestamp: bottomTimestamp,
        order: 2,
        placement: 'git-footer',
      });
    } else if (state.worktree.dirty && !hasActiveCommitSession) {
      rows.push({
        key: 'git-dirty',
        type: 'git-diff',
        title: withDetail('Uncommitted changes', worktreeSummary(state)),
        timestamp: bottomTimestamp,
        order: 2,
        placement: 'git-footer',
        onViewDiff: onViewWorktreeDiff,
        onCommitChanges: commitChangesDisabledReason ? undefined : onCommitWorktreeChanges,
        commitChangesDisabledReason,
        onDiscardChanges: discardChangesDisabledReason ? undefined : onDiscardWorktreeChanges,
        discardChangesDisabledReason,
      });
    }

    return rows;
  }

  // Merge commits, notes, and reviews into a single sorted list
  let items = $derived.by(() => {
    const nowMs = minuteNow.now();
    const all: DisplayItem[] = [];
    const commitAnchors = new Map<string, CommitAnchor>();
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
        secondaryMeta = failedArtifactSubtitle(commit.completionReason, 'commit');
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

      const showAuthor = type === 'commit' && !isDeleting && !!commit.author && !commit.isOwnCommit;

      all.push({
        key: commit.sha || `pending-${commit.sessionId || commit.timestamp}`,
        type,
        title: stripXmlTags(commit.subject),
        meta: isDeleting ? 'Deleting...' : secondaryMeta,
        secondaryMeta: isDeleting || isRunning ? undefined : commit.shortSha || undefined,
        tertiaryMeta: showAuthor ? commit.author : undefined,
        deleting: isDeleting,
        timestamp: commit.timestamp,
        order: commit.order,
        sessionId: commit.sessionId ?? undefined,
        commitSha: commit.sha || undefined,
        commitId: commit.id ?? undefined,
        deleteDisabledReason: isDeleting
          ? 'Deleting...'
          : type === 'commit'
            ? (gitActionDisabledReason ?? undefined)
            : undefined,
        completionReason: commit.completionReason,
        hashtagRef: type === 'commit' ? `#commit:${commit.sha}` : undefined,
      });

      if (type === 'commit' && commit.sha) {
        commitAnchors.set(commit.sha, {
          timestamp: commit.timestamp,
          order: commit.order,
          shortSha: commit.shortSha || undefined,
        });
      }
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
        secondaryMeta = failedArtifactSubtitle(note.completionReason, 'note');
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
        noteUpdatedAt: note.updatedAt,
        deleteDisabledReason: isDeleting ? 'Deleting...' : undefined,
        completionReason: note.completionReason,
        hashtagRef: type === 'note' ? `#note:${note.id}` : undefined,
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
        meta = failedArtifactSubtitle(review.completionReason, 'review');
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
        hashtagRef: type === 'review' ? `#review:${review.id}` : undefined,
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

    if (timeline.gitState) {
      all.push(...gitStateRows(timeline.gitState, commitAnchors));
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

  let normalItems = $derived(items.filter((item) => item.placement !== 'git-footer'));
  let gitFooterItems = $derived(items.filter((item) => item.placement === 'git-footer'));
  let timelineContextMenuActions = $derived.by(() => {
    const actions: TimelineContextMenuAction[] = [];
    for (const item of normalItems) {
      const action = contextMenuActionForItem(item);
      if (action) actions.push(action);
    }
    for (const item of gitFooterItems) {
      const action = contextMenuActionForItem(item);
      if (action) actions.push(action);
    }
    return actions;
  });
  let actionFooterVisible = $derived(
    !!onNewNote || !!onNewCommit || !!onNewReview || !!footerActions
  );

  /** True when the timeline has no content and action buttons should be enlarged. */
  let actionButtonsEnlarged = $derived(
    items.length === 0 && pendingDropNotes.length === 0 && pendingItems.length === 0
  );

  // ── Handlers ──────────────────────────────────────────────────────────

  function handleItemClick(item: DisplayItem) {
    if (item.type === 'commit' && item.commitSha && onCommitClick) {
      onCommitClick(item.commitSha);
    } else if (item.type === 'note' && item.noteId && onNoteClick) {
      onNoteClick({
        noteId: item.noteId,
        title: item.noteTitle ?? '',
        content: item.noteContent ?? '',
        sessionId: item.sessionId,
        updatedAt: item.noteUpdatedAt,
      });
    } else if (item.type === 'review' && item.reviewId && onReviewClick) {
      onReviewClick(item.reviewId);
    } else if (item.type === 'image' && item.imageId && onImageClick) {
      onImageClick(item.imageId);
    }
  }

  function isResumable(item: DisplayItem): boolean {
    return !!item.sessionId && isResumableReason(item.completionReason) && !item.deleting;
  }

  function hasContextMenuAction(item: DisplayItem): boolean {
    return !!item.commitSha || (!!item.hashtagRef && !!onNewSessionReferring) || isDeletable(item);
  }

  function contextMenuActionForItem(item: DisplayItem): TimelineContextMenuAction | null {
    if (!hasContextMenuAction(item)) return null;
    const deleteDisabledReason = isDeletable(item) ? item.deleteDisabledReason : undefined;

    return {
      key: item.key,
      commitSha: item.commitSha,
      hashtagRef: item.hashtagRef,
      deleteDisabledReason,
      onDelete:
        isDeletable(item) && !deleteDisabledReason
          ? (opts) => handleDeleteClick(item, opts)
          : undefined,
    };
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

  function isDeletable(item: DisplayItem): boolean {
    if (item.type === 'commit') return !!item.commitSha && !!onDeleteCommit;
    if (
      item.type === 'failed-commit' ||
      item.type === 'pending-commit' ||
      item.type === 'queued-commit'
    ) {
      return !!item.commitId && !!onDeletePendingCommit;
    }
    if (
      item.type === 'note' ||
      item.type === 'failed-note' ||
      item.type === 'generating-note' ||
      item.type === 'queued-note'
    ) {
      return !!item.noteId && !!onDeleteNote;
    }
    if (
      item.type === 'review' ||
      item.type === 'failed-review' ||
      item.type === 'generating-review' ||
      item.type === 'queued-review'
    ) {
      return !!item.reviewId && !!onDeleteReview;
    }
    if (item.type === 'image') return !!item.imageId && !!onDeleteImage;
    return false;
  }
</script>

{#if items.length === 0 && !actionFooterVisible && pendingDropNotes.length === 0 && pendingItems.length === 0}
  <p class="no-items">No commits or notes yet</p>
{:else}
  <!-- Unified timeline (vertical) -->
  <TimelineContextMenu actions={timelineContextMenuActions} {onNewSessionReferring}>
    <div class="timeline">
      {#each normalItems as item, index (item.key)}
        <div>
          <TimelineRow
            type={item.type}
            title={item.title}
            titleHtml={item.titleHtml}
            meta={item.meta}
            secondaryMeta={item.secondaryMeta}
            tertiaryMeta={item.tertiaryMeta}
            badges={item.badges}
            onPullClick={item.onPull}
            pullDisabledReason={item.pullDisabledReason}
            onPushClick={item.onPush}
            pushDisabledReason={item.pushDisabledReason}
            onRebaseClick={item.onRebase}
            rebaseDisabledReason={item.rebaseDisabledReason}
            onForcePushClick={item.onForcePush}
            forcePushDisabledReason={item.forcePushDisabledReason}
            forcePushing={item.forcePushing}
            onResetToOriginClick={item.onResetToOrigin}
            resetToOriginDisabledReason={item.resetToOriginDisabledReason}
            resettingToOrigin={item.resettingToOrigin}
            pushing={item.pushing}
            onViewDiffClick={item.onViewDiff}
            onCommitChangesClick={item.onCommitChanges}
            commitChangesDisabledReason={item.commitChangesDisabledReason}
            onDiscardChangesClick={item.onDiscardChanges}
            discardChangesDisabledReason={item.discardChangesDisabledReason}
            deleting={item.deleting}
            isLast={index === normalItems.length - 1 &&
              pendingDropNotes.length === 0 &&
              pendingItems.length === 0 &&
              gitFooterItems.length === 0 &&
              !error &&
              !actionFooterVisible}
            sessionId={item.sessionId}
            deleteDisabledReason={isDeletable(item) ? item.deleteDisabledReason : undefined}
            contextMenuKey={hasContextMenuAction(item) ? item.key : undefined}
            showConnector={item.showConnector}
            {onSessionClick}
            onItemClick={() => handleItemClick(item)}
            onDeleteClick={!isDeletable(item) || item.deleteDisabledReason
              ? undefined
              : (opts) => handleDeleteClick(item, opts)}
            onStartClick={item.type.startsWith('queued-') && !hasActiveSession
              ? onStartQueued
              : undefined}
            onResumeClick={isResumable(item) && onResumeClick && item.sessionId && !hasActiveSession
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
              gitFooterItems.length === 0 &&
              !error &&
              !actionFooterVisible}
          />
        </div>
      {/each}
      {#each pendingItems as item, index (item.key)}
        <div in:slide={{ duration: 200 }} out:maybeSlide={{ sessionId: item.sessionId }}>
          <TimelineRow
            type={item.type}
            title={item.title}
            titleHtml={hashtagItems.length > 0 && hasHashtagTokens(item.title)
              ? renderHashtagTokens(item.title, hashtagItems)
              : undefined}
            secondaryMeta={item.sessionId
              ? (liveSessionHints[item.sessionId] ??
                item.secondaryMeta ??
                fallbackHintForPendingType(item.type))
              : item.secondaryMeta}
            isLast={index === pendingItems.length - 1 &&
              gitFooterItems.length === 0 &&
              !error &&
              !actionFooterVisible}
          />
        </div>
      {/each}
      {#each gitFooterItems as item, index (item.key)}
        <div>
          <TimelineRow
            type={item.type}
            title={item.title}
            titleHtml={item.titleHtml}
            meta={item.meta}
            secondaryMeta={item.secondaryMeta}
            tertiaryMeta={item.tertiaryMeta}
            badges={item.badges}
            onPullClick={item.onPull}
            pullDisabledReason={item.pullDisabledReason}
            onPushClick={item.onPush}
            pushDisabledReason={item.pushDisabledReason}
            onRebaseClick={item.onRebase}
            rebaseDisabledReason={item.rebaseDisabledReason}
            onForcePushClick={item.onForcePush}
            forcePushDisabledReason={item.forcePushDisabledReason}
            forcePushing={item.forcePushing}
            onResetToOriginClick={item.onResetToOrigin}
            resetToOriginDisabledReason={item.resetToOriginDisabledReason}
            resettingToOrigin={item.resettingToOrigin}
            pushing={item.pushing}
            onViewDiffClick={item.onViewDiff}
            onCommitChangesClick={item.onCommitChanges}
            commitChangesDisabledReason={item.commitChangesDisabledReason}
            onDiscardChangesClick={item.onDiscardChanges}
            discardChangesDisabledReason={item.discardChangesDisabledReason}
            deleting={item.deleting}
            isLast={index === gitFooterItems.length - 1 && !error}
            sessionId={item.sessionId}
            deleteDisabledReason={isDeletable(item) ? item.deleteDisabledReason : undefined}
            contextMenuKey={hasContextMenuAction(item) ? item.key : undefined}
            showConnector={item.showConnector}
            {onSessionClick}
            onItemClick={() => handleItemClick(item)}
            onDeleteClick={!isDeletable(item) || item.deleteDisabledReason
              ? undefined
              : (opts) => handleDeleteClick(item, opts)}
            onStartClick={item.type.startsWith('queued-') && !hasActiveSession
              ? onStartQueued
              : undefined}
            onResumeClick={isResumable(item) && onResumeClick && item.sessionId && !hasActiveSession
              ? () => onResumeClick!(item.sessionId!)
              : undefined}
          />
        </div>
      {/each}
      {#if error}
        <div>
          <TimelineRow
            type="load-error"
            title="Failed to load commits"
            secondaryMeta={error}
            isLast={!actionFooterVisible}
            onRetryClick={onRetry}
          />
        </div>
      {/if}
      {#if actionFooterVisible}
        <div class="footer-row" class:footer-row-enlarged={actionButtonsEnlarged}>
          <div
            class="footer-left-actions"
            class:footer-left-actions-enlarged={actionButtonsEnlarged}
          >
            {#if onNewNote}
              <span class={actionButtonsEnlarged ? 'inline-flex flex-1' : 'inline-flex'}>
                <Button
                  variant="ghost"
                  onclick={onNewNote}
                  disabled={newSessionDisabled}
                  aria-label="New note"
                  class={[
                    'inline-flex items-center font-medium transition-[color,background-color,border-color,box-shadow,opacity] duration-300 disabled:opacity-30 disabled:cursor-not-allowed',
                    '[&_svg]:transition-colors [&_svg]:duration-300',
                    actionButtonsEnlarged
                      ? 'flex-1 justify-center gap-2 px-1.5 py-2.5 h-auto rounded-lg border border-solid border-transparent bg-[var(--bg-elevated)] text-sm hover:not-disabled:bg-[var(--note-bg)] hover:not-disabled:text-[var(--note-color)] [&_svg]:!size-[18px] [&_svg]:text-[var(--note-color)]'
                      : 'gap-[5px] px-2.5 h-8 rounded-md border border-dashed border-[var(--border-subtle)] bg-transparent text-xs hover:not-disabled:border-[var(--note-color)] hover:not-disabled:bg-[var(--note-bg)] hover:not-disabled:text-[var(--note-color)] [&_svg]:!size-[13px] [&_svg]:text-[var(--note-color)] @max-[480px]/timeline:gap-0.5 @max-[480px]/timeline:px-1.5',
                  ]}
                >
                  <FileText
                    class={actionButtonsEnlarged
                      ? ''
                      : '@max-3xl/timeline:hidden @max-[480px]/timeline:inline-block'}
                    size={18}
                  />
                  <Plus
                    class={actionButtonsEnlarged
                      ? 'hidden'
                      : 'hidden @max-3xl/timeline:inline-block @max-[480px]/timeline:!size-[10px]'}
                    size={18}
                  />
                  <span class={!actionButtonsEnlarged ? '@max-3xl/timeline:hidden' : ''}
                    >New note</span
                  >
                  <span
                    class={[
                      'hidden',
                      !actionButtonsEnlarged &&
                        '@max-3xl/timeline:inline @max-[480px]/timeline:hidden',
                    ]}>Note</span
                  >
                </Button>
              </span>
            {/if}
            {#if onNewCommit}
              <span class={actionButtonsEnlarged ? 'inline-flex flex-1' : 'inline-flex'}>
                <Button
                  variant="ghost"
                  onclick={onNewCommit}
                  disabled={newSessionDisabled}
                  aria-label="New commit"
                  class={[
                    'inline-flex items-center font-medium transition-[color,background-color,border-color,box-shadow,opacity] duration-300 disabled:opacity-30 disabled:cursor-not-allowed',
                    '[&_svg]:transition-colors [&_svg]:duration-300',
                    actionButtonsEnlarged
                      ? 'flex-1 justify-center gap-2 px-1.5 py-2.5 h-auto rounded-lg border border-solid border-transparent bg-[var(--bg-elevated)] text-sm hover:not-disabled:bg-[var(--commit-bg)] hover:not-disabled:text-[var(--commit-color)] [&_svg]:!size-[18px] [&_svg]:text-[var(--commit-color)]'
                      : 'gap-[5px] px-2.5 h-8 rounded-md border border-dashed border-[var(--border-subtle)] bg-transparent text-xs hover:not-disabled:border-[var(--commit-color)] hover:not-disabled:bg-[var(--commit-bg)] hover:not-disabled:text-[var(--commit-color)] [&_svg]:!size-[13px] [&_svg]:text-[var(--commit-color)] @max-[480px]/timeline:gap-0.5 @max-[480px]/timeline:px-1.5',
                  ]}
                >
                  <GitCommitVertical
                    class={actionButtonsEnlarged
                      ? ''
                      : '@max-3xl/timeline:hidden @max-[480px]/timeline:inline-block'}
                    size={18}
                  />
                  <Plus
                    class={actionButtonsEnlarged
                      ? 'hidden'
                      : 'hidden @max-3xl/timeline:inline-block @max-[480px]/timeline:!size-[10px]'}
                    size={18}
                  />
                  <span class={!actionButtonsEnlarged ? '@max-3xl/timeline:hidden' : ''}
                    >New commit</span
                  >
                  <span
                    class={[
                      'hidden',
                      !actionButtonsEnlarged &&
                        '@max-3xl/timeline:inline @max-[480px]/timeline:hidden',
                    ]}>Commit</span
                  >
                </Button>
              </span>
            {/if}
            {#if onNewReview}
              <span class={actionButtonsEnlarged ? 'inline-flex flex-1' : 'inline-flex'}>
                <Button
                  variant="ghost"
                  onclick={(e) => onNewReview?.(e)}
                  disabled={newSessionDisabled}
                  aria-label="New code review"
                  class={[
                    'inline-flex items-center font-medium transition-[color,background-color,border-color,box-shadow,opacity] duration-300 disabled:opacity-30 disabled:cursor-not-allowed',
                    '[&_svg]:transition-colors [&_svg]:duration-300',
                    actionButtonsEnlarged
                      ? 'flex-1 justify-center gap-2 px-1.5 py-2.5 h-auto rounded-lg border border-solid border-transparent bg-[var(--bg-elevated)] text-sm hover:not-disabled:bg-[var(--review-bg)] hover:not-disabled:text-[var(--review-color)] [&_svg]:!size-[18px] [&_svg]:text-[var(--review-color)]'
                      : 'gap-[5px] px-2.5 h-8 rounded-md border border-dashed border-[var(--border-subtle)] bg-transparent text-xs hover:not-disabled:border-[var(--review-color)] hover:not-disabled:bg-[var(--review-bg)] hover:not-disabled:text-[var(--review-color)] [&_svg]:!size-[13px] [&_svg]:text-[var(--review-color)] @max-[480px]/timeline:gap-0.5 @max-[480px]/timeline:px-1.5',
                  ]}
                >
                  <FileSearch
                    class={actionButtonsEnlarged
                      ? ''
                      : '@max-3xl/timeline:hidden @max-[480px]/timeline:inline-block'}
                    size={18}
                  />
                  <Plus
                    class={actionButtonsEnlarged
                      ? 'hidden'
                      : 'hidden @max-3xl/timeline:inline-block @max-[480px]/timeline:!size-[10px]'}
                    size={18}
                  />
                  <span class={!actionButtonsEnlarged ? '@max-3xl/timeline:hidden' : ''}
                    >New code review</span
                  >
                  <span
                    class={[
                      'hidden',
                      !actionButtonsEnlarged &&
                        '@max-3xl/timeline:inline @max-[480px]/timeline:hidden',
                    ]}>Code review</span
                  >
                </Button>
              </span>
            {/if}
          </div>
          {#if footerActions && !actionButtonsEnlarged}
            {@render footerActions()}
          {/if}
        </div>
      {/if}
    </div>
  </TimelineContextMenu>
{/if}

<style>
  /* ── Timeline ────────────────────────────────────────────────────────── */

  .timeline {
    display: flex;
    flex-direction: column;
    container-type: inline-size;
    container-name: timeline;
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
</style>
