<!--
  DiffModal.svelte — Diff viewer modal with file tree sidebar

  Opened from a branch card or timeline item. Contains:
  - DiffViewer (renders the selected file's diff)
  - File sidebar on the right with tree view, review status, reference files, comments

  Props: branchId, commitSha (optional), scope, onClose.

  State management:
  - diffViewerState: file list + on-demand diff cache
  - reviewState: lazy review creation, comments, reviewed paths, reference files
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import GitCommitHorizontal from '@lucide/svelte/icons/git-commit-horizontal';
  import PanelRightOpen from '@lucide/svelte/icons/panel-right-open';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { Button } from '$lib/components/ui/button';
  import { toast } from 'svelte-sonner';
  import { formatRelativeTimeSeconds } from '../../shared/relativeTime.svelte';
  import { DiffViewer, CrossFileSearchBar } from '@builderbot/diff-viewer/components';
  import DiffCommentsSection from './DiffCommentsSection.svelte';
  import DiffFileTreeSection from './DiffFileTreeSection.svelte';
  import DiffCommitSessionLauncher from './DiffCommitSessionLauncher.svelte';
  import DiffReferenceSection from './DiffReferenceSection.svelte';
  import ReviewCommentActions from './ReviewCommentActions.svelte';
  import NewSessionModal from '../sessions/NewSessionModal.svelte';
  import SessionModal from '../sessions/SessionModal.svelte';
  import NoteModal from '../notes/NoteModal.svelte';
  import { onBranchSessionStatus } from '../../services/branchEventService';
  import { preferences } from '../settings/preferences.svelte';
  import { createDiffViewerState } from './diffViewerState.svelte';
  import { createReviewState } from './reviewState.svelte';
  import { createSearchState } from '@builderbot/diff-viewer/state';
  import {
    setupDiffKeyboardNav,
    createSearchNavigationHandlers,
    createSearchInitializationTracker,
    createFileSelectionWithSearch,
  } from '@builderbot/diff-viewer/utils';
  import type {
    AcpConfigSelection,
    Branch,
    BranchSessionType,
    BranchSessionLaunchStatus,
    Comment,
    CommentActionContext,
    CommentSessionState,
    CommitTimelineItem,
    HashtagItem,
    SessionStatus,
    SmartDiffAnnotation,
    Span,
  } from '../../types';
  import type { DiffScope } from '../../commands';
  import { findFreshAutoReview, getSession } from '../../commands';
  import * as commands from '../../api/commands';
  import { startOrQueueBranchSessionWithPending } from '../branches/branchSessionLaunch.svelte';
  import { buildBranchHashtagItems } from '../sessions/hashtagItems';
  import {
    buildFileEntries,
    buildTree,
    buildGithubCommentUrl,
    compactTree,
    formatLineRange,
    pathsMatch,
    truncateText,
    type FileEntry,
    type TreeNode,
  } from './diffModalHelpers';
  import TopBarPortal from '../layout/TopBarPortal.svelte';
  import { openDiffRoute } from '../layout/navigation.svelte';
  import {
    disabledReferenceNav,
    pushReferenceEntry,
    resolveHashtagReference,
    type HashtagClickInfo,
    type ReferenceDiffContext,
    type ReferenceHistoryEntry,
  } from '../references/referenceHistory.svelte';

  // ==========================================================================
  // Props
  // ==========================================================================

  interface Props {
    branchId: string;
    /** Project ID for scoping hashtag references to include project-level notes. */
    projectId?: string | null;
    /** Optional — for branch scope, resolved automatically. */
    commitSha?: string;
    scope?: DiffScope;
    /** When set, opens a specific existing review by ID instead of searching by triple. */
    reviewId?: string;
    /** Label for the before pane header. */
    beforeLabel?: string;
    /** Label for the after pane header. */
    afterLabel?: string;
    /** When true, hides commenting, reference files, and review status. */
    readonly?: boolean;
    /** Available commits for the context switcher dropdown. */
    commits?: CommitTimelineItem[];
    /** Base branch name (e.g. "main") for label display when switching to branch scope. */
    baseBranchLabel?: string;
    /** Branch name for label display when switching to branch scope. */
    branchLabel?: string;
    /** Project name to display in title bar. */
    projectName?: string;
    /** GitHub repo (e.g., "owner/repo") to display in title bar. */
    githubRepo?: string;
    /** Subpath within the repo to display in title bar. */
    subpath?: string | null;
    onClose: () => void;
  }

  let {
    branchId,
    projectId,
    commitSha,
    scope = 'branch',
    reviewId,
    beforeLabel = 'base',
    afterLabel = 'head',
    readonly = false,
    commits,
    baseBranchLabel,
    branchLabel,
    projectName,
    githubRepo,
    subpath,
    onClose,
  }: Props = $props();

  // ==========================================================================
  // State
  // ==========================================================================

  // svelte-ignore state_referenced_locally
  const diffViewer = createDiffViewerState(branchId, scope, commitSha);

  // svelte-ignore state_referenced_locally
  const searchState = createSearchState();

  type ReviewHandle = ReturnType<typeof createReviewState>;
  let reviewHandle = $state<ReviewHandle | null>(null);

  const SMALL_DIFF_BREAKPOINT = 700;
  const MOBILE_DIFF_REST_OFFSET = 20;
  const MOBILE_DIFF_EDGE_PEEK = 28;
  const MOBILE_DIFF_REVEAL_HIT_WIDTH = 28;

  type DiffViewerScrollApi = {
    scrollBy: (side: 'before' | 'after', deltaY: number) => void;
    scrollByX: (side: 'before' | 'after', deltaX: number) => void;
    scrollByXBoth: (deltaX: number) => void;
    canScrollX: (side: 'before' | 'after') => boolean;
  };

  type DiffViewerComponentHandle = {
    flushCommentEditors: () => Promise<boolean>;
    flushAndClearCommentEditors: () => Promise<boolean>;
  };

  type MobileDiffGestureMode = 'pending' | 'vertical-scroll' | 'horizontal-scroll' | 'pane-drag';

  // Create review state once we have a resolved commitSha (skip in readonly mode).
  // NOTE: This effect's tracked dependencies are `diffViewer.state.commitSha` and
  // `reviewHandle`. When `switchDiffContext` sets `reviewHandle = null`, the effect
  // re-fires and reads `activeScope`/`activeReviewId` inside the `if` branch.
  // Those reads happen *conditionally* (only when `reviewHandle` is null), so Svelte
  // tracks them only on runs where the branch executes. This is correct today but
  // relies on Svelte 5's fine-grained conditional tracking — if that ever changes,
  // explicitly adding `activeScope`/`activeReviewId` to the dependency list would
  // be needed to keep the effect firing on context switches.
  $effect(() => {
    const sha = diffViewer.state.commitSha;
    if (sha && !reviewHandle && !readonly && activeScope !== 'worktree') {
      // svelte-ignore state_referenced_locally
      reviewHandle = createReviewState(branchId, sha, reviewableScope(), activeReviewId);
    }
  });

  // Context switcher state
  // svelte-ignore state_referenced_locally
  let activeScope = $state<DiffScope>(scope);
  // svelte-ignore state_referenced_locally
  let activeCommitSha = $state<string | undefined>(commitSha);
  // svelte-ignore state_referenced_locally
  let activeBeforeLabel = $state(beforeLabel);
  // svelte-ignore state_referenced_locally
  let activeAfterLabel = $state(afterLabel);
  // svelte-ignore state_referenced_locally
  let activeReviewId = $state<string | undefined>(reviewId);
  let showContextDropdown = $state(false);
  /** Whether a context switch is currently in-flight (disables the dropdown). */
  let switchingContext = $state(false);
  /** Tracks the active auto-review reload promise so it can be ignored on stale switches. */
  let contextSwitchGeneration = 0;

  function reviewableScope(): 'branch' | 'commit' {
    return activeScope === 'worktree' ? 'branch' : activeScope;
  }

  /** Label for the current context shown in the dropdown trigger. */
  let contextLabel = $derived(
    activeScope === 'worktree'
      ? 'Uncommitted changes'
      : activeScope === 'branch'
        ? 'All changes'
        : (commits?.find((c) => c.sha === activeCommitSha)?.subject ?? 'Commit')
  );

  /** Whether the context switcher should be shown. */
  let showContextSwitcher = $derived(activeScope !== 'worktree' && (commits?.length ?? 0) > 0);

  async function switchDiffContext(newScope: 'branch' | 'commit', newCommitSha?: string) {
    showContextDropdown = false;

    // No-op if already on this context
    if (newScope === activeScope && newCommitSha === activeCommitSha) return;
    if (!(await flushActiveCommentEditors())) return;

    const thisGeneration = ++contextSwitchGeneration;
    switchingContext = true;

    activeScope = newScope;
    activeCommitSha = newCommitSha;
    activeReviewId = undefined; // Clear specific review when switching

    // Update labels
    if (newScope === 'branch') {
      activeBeforeLabel = baseBranchLabel ?? 'base';
      activeAfterLabel = branchLabel ?? 'head';
    } else {
      activeBeforeLabel = 'parent';
      activeAfterLabel = newCommitSha?.slice(0, 7) ?? 'head';
    }

    // Always stop polling on any context switch — will restart below if needed
    stopAutoReviewPolling();
    autoReviewComments = [];

    // Reset review state — the $effect will recreate it once the new commitSha resolves
    reviewHandle = null;

    // Reset so the sidebar-order effect picks the first file after reload
    initialSelectionApplied = false;

    // Switch diff context (reloads file list)
    try {
      await diffViewer.switchContext(newScope, newCommitSha);
    } finally {
      // Bail if a newer context switch started while we were loading
      if (thisGeneration !== contextSwitchGeneration) return;
      switchingContext = false;
    }

    // If switching to branch scope, reload auto-review annotations.
    // Guard each async step against stale generations so a rapid switch
    // doesn't start polling for an already-abandoned context.
    if (newScope === 'branch') {
      loadAutoReviewAnnotations().then((review) => {
        if (thisGeneration !== contextSwitchGeneration) return;
        if (review?.sessionId) {
          getSession(review.sessionId)
            .then((session) => {
              if (thisGeneration !== contextSwitchGeneration) return;
              if (session?.status === 'running') {
                startAutoReviewPolling(review.sessionId!);
              }
            })
            .catch((e) => console.warn('Failed to check auto-review session status:', e));
        }
      });
    }
  }

  /** Index of the focused option in the dropdown (-1 = none). 0..N-1 = commits, N = "All changes". */
  let dropdownFocusIndex = $state(-1);

  /** Total number of options in the dropdown (1 for "All changes" + commit count). */
  let dropdownOptionCount = $derived(1 + (commits?.length ?? 0));

  /** Commits in chronological order (oldest first) for dropdown display and keyboard nav. */
  let reversedCommits = $derived([...(commits ?? [])].reverse());

  function openDropdown() {
    showContextDropdown = true;
    dropdownFocusIndex = -1;
  }

  function closeDropdown() {
    showContextDropdown = false;
    dropdownFocusIndex = -1;
  }

  async function handleDropdownKeydown(event: KeyboardEvent) {
    if (!showContextDropdown) return;

    if (event.key === 'ArrowDown') {
      event.preventDefault();
      dropdownFocusIndex = Math.min(dropdownFocusIndex + 1, dropdownOptionCount - 1);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      dropdownFocusIndex = Math.max(dropdownFocusIndex - 1, 0);
    } else if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      const commitCount = reversedCommits.length;
      if (dropdownFocusIndex === commitCount) {
        await switchDiffContext('branch');
      } else if (dropdownFocusIndex >= 0 && dropdownFocusIndex < commitCount) {
        const commit = reversedCommits[dropdownFocusIndex];
        if (commit) await switchDiffContext('commit', commit.sha);
      }
    } else if (event.key === 'Escape') {
      // Let the main keydown handler close the dropdown
      return;
    }
  }

  function handleClickOutsideDropdown(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (showContextDropdown && !target.closest('.context-switcher')) {
      closeDropdown();
    }
  }

  // Sidebar state
  let collapsedDirs = $state(new Set<string>());
  let copiedFeedback = $state(false);
  let selectedCommentId = $state<string | null>(null);
  let jumpToComment = $state<{ id: string; token: number } | null>(null);
  let commentJumpToken = 0;
  let jumpToLine = $state<{ lineIndex: number; token: number } | null>(null);
  let lineJumpToken = 0;
  let isSmallDiffViewport = $state(false);
  let showMobileSidebar = $state(false);
  let diffDetailEl: HTMLDivElement | null = $state(null);
  let diffViewerContainerEl: HTMLDivElement | null = $state(null);
  let mobileDiffDragX = $state(0);
  let mobileDiffPointerId: number | null = null;
  let mobileDiffStartX = 0;
  let mobileDiffStartY = 0;
  let mobileDiffLastX = 0;
  let mobileDiffStartDragX = 0;
  let mobileDiffIsDragging = $state(false);
  let mobileDiffGestureMode: MobileDiffGestureMode = 'pending';
  let mobileDiffCanDragPane = false;
  let mobileDiffCanScrollCode = false;
  let mobileDiffMarkdownScrollEl: HTMLElement | null = null;
  let mobileDiffLastY = 0;
  let diffViewerScrollApi: DiffViewerScrollApi | null = $state(null);
  let diffViewerComponent: DiffViewerComponentHandle | null = $state(null);
  let mobileDiffStyle = $derived(
    `--mobile-diff-drag-x: ${mobileDiffDragX}px;` +
      `--mobile-diff-rest-offset: ${MOBILE_DIFF_REST_OFFSET}px;` +
      `--mobile-diff-edge-peek: ${MOBILE_DIFF_EDGE_PEEK}px;`
  );

  async function flushActiveCommentEditors(): Promise<boolean> {
    return (await diffViewerComponent?.flushAndClearCommentEditors()) ?? true;
  }

  function flushCommentEditorsOnDestroy() {
    void diffViewerComponent?.flushCommentEditors();
  }

  async function closeDiffModal() {
    if (!(await flushActiveCommentEditors())) return;
    onClose();
  }

  async function prepareCommitSessionStart(): Promise<boolean> {
    return await flushActiveCommentEditors();
  }

  // (No confirmation dialogs — soft delete is reversible)

  // Annotation reveal state (hold A to reveal)
  let annotationsRevealed = $state(false);

  // Auto review state (branch-scope only)
  let autoReviewComments = $state<Comment[]>([]);
  let autoReviewPollTimer: ReturnType<typeof setInterval> | null = null;

  async function loadAutoReviewAnnotations() {
    try {
      const review = await findFreshAutoReview(branchId);
      if (!review) {
        autoReviewComments = [];
        return review;
      }
      autoReviewComments = review.comments.filter((c) => c.commentType === 'information');
      return review;
    } catch (e) {
      console.error('Failed to load auto review annotations:', e);
      autoReviewComments = [];
      return null;
    }
  }

  function startAutoReviewPolling(sessionId: string) {
    stopAutoReviewPolling();
    autoReviewPollTimer = setInterval(async () => {
      const review = await loadAutoReviewAnnotations();
      // Check if session is still running; if not, stop polling
      if (!review?.sessionId) {
        stopAutoReviewPolling();
        return;
      }
      try {
        const session = await getSession(sessionId);
        if (!session || session.status !== 'running') {
          stopAutoReviewPolling();
        }
      } catch {
        stopAutoReviewPolling();
      }
    }, 4000);
  }

  function stopAutoReviewPolling() {
    if (autoReviewPollTimer !== null) {
      clearInterval(autoReviewPollTimer);
      autoReviewPollTimer = null;
    }
  }

  // Load auto review annotations for branch-scope diffs (no specific reviewId)
  // svelte-ignore state_referenced_locally
  if (activeScope === 'branch' && !reviewId) {
    loadAutoReviewAnnotations().then((review) => {
      if (review?.sessionId) {
        // Check if the session is still running to start polling
        getSession(review.sessionId)
          .then((session) => {
            if (session?.status === 'running') {
              startAutoReviewPolling(review.sessionId!);
            }
          })
          .catch((e) => console.warn('Failed to check auto-review session status:', e));
      }
    });
  }

  onDestroy(() => {
    flushCommentEditorsOnDestroy();
    stopAutoReviewPolling();
  });

  // ==========================================================================
  // Branch data (for PR info and session launching)
  // ==========================================================================

  let branch = $state<Branch | null>(null);
  let hashtagItems = $state<HashtagItem[]>([]);

  onMount(() => {
    commands
      .getBranch(branchId)
      .then((b) => {
        if (b) branch = b;
      })
      .catch((e) => console.warn('Failed to load branch:', e));
  });

  let isRemote = $derived(branch?.branchType === 'remote');
  let hasPr = $derived(branch?.prNumber != null);
  let referenceDiffContext = $derived<ReferenceDiffContext>({
    branchId,
    projectId,
    commits,
    baseBranchLabel,
    branchLabel,
    projectName,
    githubRepo,
    subpath,
  });

  $effect(() => {
    let stale = false;
    buildBranchHashtagItems(branchId, projectId ?? null, {
      branchName: branchLabel,
      repoSlug: githubRepo,
      repoSubpath: subpath,
    })
      .then((items) => {
        if (!stale) hashtagItems = items;
      })
      .catch(() => {
        if (!stale) hashtagItems = [];
      });
    return () => {
      stale = true;
    };
  });

  // ==========================================================================
  // New Session Modal state
  // ==========================================================================

  let showNewSessionModal = $state(false);
  let newSessionMode = $state<BranchSessionType>('note');
  let newSessionPrefill = $state('');
  /** The comment that opened the New Session dialog, so submit knows the origin. */
  let newSessionComment = $state<Comment | null>(null);

  // ==========================================================================
  // Comment ↔ session links (stateful Note/Commit buttons)
  // ==========================================================================

  /** Linked session dialogs opened from a comment's Note/Commit button. */
  let openSessionId = $state<string | null>(null);
  let openNote = $state<{
    noteId?: string;
    title: string;
    content: string;
    sessionId?: string;
    noteUpdatedAt?: number;
    chatOpen?: boolean;
  } | null>(null);

  /** Live status for every comment-linked session id (note + commit). */
  let sessionStatusById = $state(new Map<string, SessionStatus>());
  /** Linked session ids already fetched, to dedupe getSession calls. */
  const requestedSessionIds = new Set<string>();

  function buildCommentPrompt(comment: Comment, mode: 'note' | 'commit'): string {
    const reviewId = reviewHandle?.state.review?.id ?? activeReviewId;
    const reviewRef = reviewId ? `Re: #review:${reviewId}\n` : '';
    const tail = mode === 'note' ? 'Plan a fix for this' : 'Fix this';
    return `${reviewRef}Code review comment:\n> ${comment.path} ${formatLineRange(comment.span)}\n> ${comment.content}\n\n${tail}`;
  }

  async function launchSessionDirectly(comment: Comment, mode: BranchSessionType) {
    const prompt = buildCommentPrompt(comment, mode as 'note' | 'commit');
    const launchContext = {
      source: 'diff_viewer' as const,
      scope: reviewableScope(),
      commitSha: diffViewer.state.commitSha ?? '',
      reviewId: activeReviewId ?? null,
    };

    const result = await startOrQueueBranchSessionWithPending({
      branchId,
      isRemote,
      mode,
      prompt,
      launchContext,
      onTimelineRefresh: () => commands.invalidateBranchTimeline(branchId),
      errorTitle: `Unable to start ${mode} session`,
    });
    if (result) {
      linkCommentToSession(comment, mode, result.sessionId, result.sessionStatus);
    }
  }

  function handleNewNote(comment: Comment, event: MouseEvent) {
    // Route by the linked note session's state (idle → start, queued/running →
    // open note chat, completed → open note).
    const state = getCommentNoteState(comment);
    if ((state === 'queued' || state === 'running') && comment.noteSessionId) {
      openNoteChat(comment.noteSessionId);
      return;
    }
    if (state === 'completed' && comment.noteSessionId) {
      void openCompletedNote(comment.noteSessionId);
      return;
    }
    if (event.altKey) {
      launchSessionDirectly(comment, 'note');
      return;
    }
    newSessionComment = comment;
    newSessionMode = 'note';
    newSessionPrefill = buildCommentPrompt(comment, 'note');
    showNewSessionModal = true;
  }

  function openNoteChat(sessionId: string) {
    openNote = {
      title: '',
      content: '',
      sessionId,
      chatOpen: true,
    };
  }

  function handleNewCommit(comment: Comment, event: MouseEvent) {
    // Route by the linked commit session's state (idle → start, queued/running →
    // open session, completed → show the commit in the open diff viewer).
    const state = getCommentCommitState(comment);
    if ((state === 'queued' || state === 'running') && comment.commitSessionId) {
      openSessionId = comment.commitSessionId;
      return;
    }
    if (state === 'completed' && comment.commitSessionId) {
      void openCompletedCommit(comment.commitSessionId);
      return;
    }
    if (event.altKey) {
      launchSessionDirectly(comment, 'commit');
      return;
    }
    newSessionComment = comment;
    newSessionMode = 'commit';
    newSessionPrefill = buildCommentPrompt(comment, 'commit');
    showNewSessionModal = true;
  }

  async function handleSessionModalSubmit(data: {
    prompt: string;
    mode: BranchSessionType;
    imageIds: string[];
    provider?: string;
    acpConfigSelection?: AcpConfigSelection | null;
  }) {
    showNewSessionModal = false;
    // Capture (and clear) the origin comment before awaiting so a follow-up
    // dialog can't reuse a stale reference.
    const originComment = newSessionComment;
    newSessionComment = null;
    const launchContext = {
      source: 'diff_viewer' as const,
      scope: reviewableScope(),
      commitSha: diffViewer.state.commitSha ?? '',
      reviewId: activeReviewId ?? null,
    };

    const result = await startOrQueueBranchSessionWithPending({
      branchId,
      isRemote,
      mode: data.mode,
      prompt: data.prompt,
      imageIds: data.imageIds,
      provider: data.provider,
      launchContext,
      acpConfigSelection: data.acpConfigSelection,
      onTimelineRefresh: () => commands.invalidateBranchTimeline(branchId),
      errorTitle: `Unable to start ${data.mode} session`,
    });

    if (originComment && result) {
      linkCommentToSession(originComment, data.mode, result.sessionId, result.sessionStatus);
    }
  }

  /** Track which comments are currently being sent/updated on GitHub. */
  let sendingCommentIds = $state(new Set<string>());
  let githubCommentUrls = $state(new Map<string, string>());

  function getGithubCommentUrl(comment: Comment): string | null {
    return (
      githubCommentUrls.get(comment.id) ??
      buildGithubCommentUrl(comment, {
        prUrl: branch?.prUrl,
        githubRepo,
        prNumber: branch?.prNumber,
      })
    );
  }

  async function handleSendToGithub(comment: Comment) {
    if (!branch?.prNumber) return;

    if (comment.githubCommentId != null && !comment.githubCommentStale) {
      const url = getGithubCommentUrl(comment);
      if (!url) {
        toast.error('Unable to open GitHub comment', {
          description: 'The comment URL is not available.',
        });
        return;
      }

      try {
        await commands.openUrl(url);
      } catch (e) {
        toast.error('Unable to open GitHub comment', {
          description: e instanceof Error ? e.message : String(e),
        });
      }
      return;
    }

    sendingCommentIds = new Set([...sendingCommentIds, comment.id]);

    try {
      const result = await commands.postCommentToGithub(branchId, branch.prNumber, comment);
      githubCommentUrls = new Map(githubCommentUrls).set(comment.id, result.commentUrl);
      // Optimistically update the comment's GitHub tracking fields
      if (reviewHandle) {
        reviewHandle.state.comments = reviewHandle.state.comments.map((c) =>
          c.id === comment.id
            ? {
                ...c,
                githubCommentId: result.commentId,
                githubCommentType: result.commentType,
                githubCommentStale: false,
              }
            : c
        );
      }
    } catch (e) {
      toast.error('Failed to post comment', {
        description: e instanceof Error ? e.message : String(e),
        duration: Infinity,
      });
    } finally {
      sendingCommentIds = new Set([...sendingCommentIds].filter((id) => id !== comment.id));
    }
  }

  function getCommentGithubState(comment: Comment): 'idle' | 'sending' | 'sent' | 'stale' {
    if (sendingCommentIds.has(comment.id)) return 'sending';
    if (comment.githubCommentId != null) {
      return comment.githubCommentStale ? 'stale' : 'sent';
    }
    return 'idle';
  }

  /**
   * Persist + optimistically reflect a comment→session link after a successful
   * start. Mirrors the GitHub optimistic-update flow. No-op for non note/commit
   * modes (e.g. review).
   */
  function linkCommentToSession(
    comment: Comment,
    mode: BranchSessionType,
    sessionId: string,
    initialStatus: BranchSessionLaunchStatus
  ) {
    if (mode !== 'note' && mode !== 'commit') return;

    // Reflect the link + initial status immediately so the button updates.
    requestedSessionIds.add(sessionId);
    sessionStatusById = new Map(sessionStatusById).set(sessionId, initialStatus);
    if (reviewHandle) {
      reviewHandle.state.comments = reviewHandle.state.comments.map((c) =>
        c.id === comment.id
          ? mode === 'note'
            ? { ...c, noteSessionId: sessionId }
            : { ...c, commitSessionId: sessionId }
          : c
      );
    }

    // Persist in the background — the optimistic update already updated the UI.
    commands
      .linkCommentSession(comment.id, sessionId, mode)
      .catch((e) => console.warn('Failed to persist comment session link:', e));
  }

  /** Map a linked session id's status to the Note/Commit button state. */
  function sessionStateFor(sessionId: string | null): CommentSessionState {
    if (!sessionId) return 'idle';
    switch (sessionStatusById.get(sessionId)) {
      // Keep `queued` distinct from `running` so the UI can show the same
      // Clock icon the timeline rows use for queued sessions.
      case 'queued':
        return 'queued';
      case 'running':
        return 'running';
      case 'completed':
        return 'completed';
      // error/cancelled → idle so the user can retry; a dangling link (missing
      // session, status still unknown) also falls back to idle.
      default:
        return 'idle';
    }
  }

  function getCommentNoteState(comment: Comment): CommentSessionState {
    return sessionStateFor(comment.noteSessionId);
  }

  function getCommentCommitState(comment: Comment): CommentSessionState {
    return sessionStateFor(comment.commitSessionId);
  }

  /** Resolve a completed note session's note and open the NoteModal. */
  async function openCompletedNote(sessionId: string) {
    try {
      const note = await commands.getBranchNoteBySession(sessionId);
      if (!note) {
        // Link points at a note session whose note row is not available yet.
        openNoteChat(sessionId);
        return;
      }
      openNote = {
        noteId: note.id,
        title: note.title,
        content: note.content,
        sessionId,
        noteUpdatedAt: note.updatedAt,
      };
    } catch (e) {
      toast.error('Unable to open note', {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  }

  /** Resolve a completed commit session's sha and show it in the open viewer. */
  async function openCompletedCommit(sessionId: string) {
    try {
      const sha = await commands.getBranchCommitBySession(sessionId);
      if (!sha) {
        // Commit hasn't landed yet (pending sha) — show the session instead.
        openSessionId = sessionId;
        return;
      }
      await switchDiffContext('commit', sha);
    } catch (e) {
      toast.error('Unable to show commit', {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  }

  function currentDialogReferenceEntry(): ReferenceHistoryEntry | null {
    if (openNote) {
      return {
        kind: 'note',
        noteKind: 'branch',
        id: openNote.noteId ?? openNote.sessionId ?? 'note',
        ref: openNote.noteId ? `#note:${openNote.noteId}` : `#chat:${openNote.sessionId}`,
        title: openNote.title,
        content: openNote.content,
        view: openNote.chatOpen ? 'chat' : 'note',
        sessionId: openNote.sessionId,
        noteUpdatedAt: openNote.noteUpdatedAt,
        branchId,
        projectId,
        repoDir: branch?.worktreePath,
        repoLabel: githubRepo ? { githubRepo, subpath: subpath ?? null, headRepo: null } : null,
        hashtagItems,
        diffContext: referenceDiffContext,
      };
    }

    if (openSessionId) {
      return {
        kind: 'chat',
        ref: `#chat:${openSessionId}`,
        sessionId: openSessionId,
        branchId,
        projectId,
        repoDir: branch?.worktreePath,
        repoLabel: githubRepo ? { githubRepo, subpath: subpath ?? null, headRepo: null } : null,
        hashtagItems,
        diffContext: referenceDiffContext,
      };
    }

    return null;
  }

  function closeReferenceDialogs() {
    openNote = null;
    openSessionId = null;
  }

  function handleHashtagClick(click: HashtagClickInfo) {
    const target = resolveHashtagReference(click, {
      hashtagItems,
      diffContext: referenceDiffContext,
    });
    if (!target) return;

    const current = currentDialogReferenceEntry();
    if (current) pushReferenceEntry(current);
    pushReferenceEntry(target);
    closeReferenceDialogs();

    if (target.kind === 'diff') {
      openDiffRoute(target.route);
    }
  }

  // Create tracker for search initialization
  const checkSearchInitialization = createSearchInitializationTracker({
    searchState,
    getFiles: () => diffViewer.state.files,
  });

  // ==========================================================================
  // Derived
  // ==========================================================================

  let currentDiff = $derived(diffViewer.getCurrentDiff());
  let diffViewerEmptyMessage = $derived(
    diffViewer.state.loading || diffViewer.state.loadingFile !== null
      ? 'Loading changes...'
      : undefined
  );
  let rawComments = $derived(reviewHandle?.state.comments ?? []);

  /** Normalize comment paths so they match the file list used by the diff viewer.
   *  This is done once here so downstream components can use exact equality. */
  let allComments = $derived(
    rawComments.map((c) => {
      const resolved = resolveCommentPath(c.path);
      return resolved === c.path ? c : { ...c, path: resolved };
    })
  );

  // Split AI "information" comments into annotations; everything else stays as comments
  let currentComments = $derived(allComments.filter((c) => c.commentType !== 'information'));

  $effect(() => {
    if (selectedCommentId && !currentComments.some((c) => c.id === selectedCommentId)) {
      clearCommentNavigation(selectedCommentId);
    }
    if (jumpToComment && !currentComments.some((c) => c.id === jumpToComment?.id)) {
      jumpToComment = null;
    }
  });

  // Seed the status map for every comment-linked session id when comments load.
  // `requestedSessionIds` dedupes fetches; the map is then kept live by the
  // `session-status-changed` subscription below.
  $effect(() => {
    for (const c of allComments) {
      for (const id of [c.noteSessionId, c.commitSessionId]) {
        if (!id || requestedSessionIds.has(id)) continue;
        requestedSessionIds.add(id);
        getSession(id)
          .then((session) => {
            // Dangling link (session deleted) → leave unset so it reads as idle.
            if (!session) return;
            // A live `session-status-changed` update may have landed while this
            // fetch was in flight; that value is fresher, so don't clobber it.
            if (sessionStatusById.has(id)) return;
            sessionStatusById = new Map(sessionStatusById).set(id, session.status);
          })
          .catch((e) => console.warn('Failed to load comment session status:', e));
      }
    }
  });

  // Keep linked-session statuses live while the modal is open. Mirrors the
  // auto-review polling precedent, but event-driven via the shared listener.
  $effect(() => {
    const linkedSessionIds = new Set<string>();
    for (const comment of allComments) {
      if (comment.noteSessionId) linkedSessionIds.add(comment.noteSessionId);
      if (comment.commitSessionId) linkedSessionIds.add(comment.commitSessionId);
    }

    const unlisten = onBranchSessionStatus(branchId, (payload) => {
      if (!linkedSessionIds.has(payload.sessionId)) return;
      if (sessionStatusById.get(payload.sessionId) === payload.status) return;
      sessionStatusById = new Map(sessionStatusById).set(payload.sessionId, payload.status);
    });
    return () => unlisten();
  });

  /** Convert "information" comments to SmartDiffAnnotation for the overlay system.
   *  Merges annotations from both the user's review and the latest auto review. */
  let currentAnnotations = $derived<SmartDiffAnnotation[]>([
    ...allComments
      .filter((c) => c.commentType === 'information')
      .map((c) => ({
        id: c.id,
        file_path: c.path,
        after_span: { start: c.span.start, end: c.span.end },
        content: c.content,
        category: 'explanation' as const,
      })),
    ...autoReviewComments.map((c) => ({
      id: c.id,
      file_path: c.path,
      after_span: { start: c.span.start, end: c.span.end },
      content: c.content,
      category: 'explanation' as const,
    })),
  ]);

  let fileEntries = $derived(
    buildFileEntries(
      diffViewer.state.files,
      reviewHandle?.state.reviewedPaths ?? [],
      currentComments
    )
  );
  let needsReview = $derived(fileEntries.filter((f) => !f.isReviewed));
  let reviewed = $derived(fileEntries.filter((f) => f.isReviewed));
  let readonlyTree = $derived(compactTree(buildTree(fileEntries)));
  let needsReviewTree = $derived(compactTree(buildTree(needsReview)));
  let reviewedTree = $derived(compactTree(buildTree(reviewed)));
  /** Flatten tree nodes depth-first to get the visual file order in the sidebar. */
  let revealedAnnotations = $derived(annotationsRevealed ? currentAnnotations : []);

  function flattenTreeFiles(nodes: TreeNode[]): FileEntry[] {
    const result: FileEntry[] = [];
    for (const node of nodes) {
      if (node.isDir) {
        result.push(...flattenTreeFiles(node.children));
      } else if (node.file) {
        result.push(node.file);
      }
    }
    return result;
  }

  /** Files in sidebar visual order: needs-review tree then reviewed tree (or readonly tree). */
  let orderedFiles = $derived(
    readonly
      ? flattenTreeFiles(readonlyTree)
      : [...flattenTreeFiles(needsReviewTree), ...flattenTreeFiles(reviewedTree)]
  );

  // Once files finish loading, override the initial selection with the file
  // containing the first comment (if any), falling back to the first sidebar
  // file otherwise. For non-readonly reviews, also wait for review state to
  // load so the comment list and needs-review / reviewed split are accurate.
  let initialSelectionApplied = false;
  $effect(() => {
    if (
      !diffViewer.state.loading &&
      orderedFiles.length > 0 &&
      !initialSelectionApplied &&
      (readonly || !reviewHandle || !reviewHandle.state.loading)
    ) {
      initialSelectionApplied = true;
      const firstComment = currentComments[0];
      if (firstComment) {
        selectedCommentId = firstComment.id;
        commentJumpToken += 1;
        jumpToComment = { id: firstComment.id, token: commentJumpToken };
        diffViewer.selectFile(resolveCommentPath(firstComment.path));
      } else {
        diffViewer.selectFile(orderedFiles[0].path);
      }
    }
  });

  // ==========================================================================
  // Sidebar interactions
  // ==========================================================================

  // Create handler for search-aware file selection
  const handleSearchOnFileSelect = createFileSelectionWithSearch({
    searchState,
    getFiles: () => diffViewer.state.files,
  });

  async function selectFilePath(path: string): Promise<boolean> {
    if (!(await flushActiveCommentEditors())) return false;
    selectedCommentId = null;
    if (isSmallDiffViewport) showMobileSidebar = false;
    await diffViewer.selectFile(path);
    return true;
  }

  async function selectFile(file: FileEntry): Promise<boolean> {
    if (!(await flushActiveCommentEditors())) return false;
    selectedCommentId = null;
    if (isSmallDiffViewport) showMobileSidebar = false;
    handleSearchOnFileSelect(file.path);
    await diffViewer.selectFile(file.path);
    return true;
  }

  let diffViewerForFileTree = $derived({
    state: diffViewer.state,
    getCurrentDiff: () => diffViewer.getCurrentDiff(),
    selectFile: selectFilePath,
  });

  async function toggleReviewed(event: MouseEvent | KeyboardEvent, file: FileEntry) {
    event.stopPropagation();
    await reviewHandle?.toggleReviewed(file.path);
  }

  function toggleDir(path: string) {
    const newSet = new Set(collapsedDirs);
    if (newSet.has(path)) newSet.delete(path);
    else newSet.add(path);
    collapsedDirs = newSet;
  }

  function isCollapsed(path: string): boolean {
    return collapsedDirs.has(path);
  }

  function getFileName(path: string): string {
    return path.split('/').pop() || path;
  }

  function clearCommentNavigation(commentId?: string) {
    if (!commentId || selectedCommentId === commentId) {
      selectedCommentId = null;
    }
    if (!commentId || jumpToComment?.id === commentId) {
      jumpToComment = null;
    }
  }

  async function handleDeleteComment(commentId: string) {
    clearCommentNavigation(commentId);
    await reviewHandle?.deleteComment(commentId);
  }

  async function handleDeleteAllComments() {
    clearCommentNavigation();
    await reviewHandle?.deleteAllComments();
  }

  async function handleRestoreComment(commentId: string) {
    await reviewHandle?.restoreComment(commentId);
  }

  async function handleCopyComments() {
    if (!currentComments.length) return;
    // Build simple markdown
    const lines: string[] = [];
    for (const c of currentComments) {
      lines.push(`**${c.path}** ${formatLineRange(c.span)}`);
      lines.push(c.content);
      lines.push('');
    }
    try {
      await navigator.clipboard.writeText(lines.join('\n'));
      copiedFeedback = true;
      setTimeout(() => (copiedFeedback = false), 1500);
    } catch (e) {
      console.error('Failed to copy:', e);
    }
  }

  async function handleRemoveReferenceFile(path: string) {
    await reviewHandle?.removeReferenceFile(path);
  }

  /** Resolve a comment's path to a file-list path.
   *  Falls back to the comment's own path if no match is found. */
  function resolveCommentPath(commentPath: string): string {
    const files = diffViewer.state.files;
    for (const f of files) {
      const fp = f.after ?? f.before ?? '';
      if (pathsMatch(commentPath, fp)) return fp;
    }
    return commentPath;
  }

  async function handleSelectComment(comment: Comment) {
    if (!(await flushActiveCommentEditors())) return;
    selectedCommentId = comment.id;
    if (isSmallDiffViewport) showMobileSidebar = false;
    const resolvedPath = resolveCommentPath(comment.path);
    // Set jumpToComment BEFORE awaiting selectFile so the auto-scroll effect
    // sees the pending token and defers to the explicit comment navigation.
    commentJumpToken += 1;
    jumpToComment = { id: comment.id, token: commentJumpToken };
    await diffViewer.selectFile(resolvedPath);
  }

  // Wrapper for search that returns the loaded diff without changing selection
  async function loadFileDiffForSearch(path: string) {
    return await diffViewer.loadFileDiff(path);
  }

  // Jump to a specific line (for search results)
  function handleJumpToLine(lineIndex: number) {
    if (isSmallDiffViewport) showMobileSidebar = false;
    lineJumpToken += 1;
    jumpToLine = { lineIndex, token: lineJumpToken };
  }

  // ==========================================================================
  // Comment callbacks (wired to review state)
  // ==========================================================================

  async function handleAddComment(
    path: string,
    span: Span,
    content: string
  ): Promise<Comment | null> {
    return (await reviewHandle?.addComment(path, span, content)) ?? null;
  }

  async function handleUpdateComment(commentId: string, content: string): Promise<void> {
    await reviewHandle?.updateComment(commentId, content);
  }

  async function handleDeleteCommentFromViewer(commentId: string): Promise<void> {
    clearCommentNavigation(commentId);
    await reviewHandle?.deleteComment(commentId);
  }

  // ==========================================================================
  // Keyboard
  // ==========================================================================

  /** True when the event target is an editable element (input, textarea, contentEditable). */
  function isEditableTarget(target: EventTarget | null): boolean {
    return (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement ||
      (target instanceof HTMLElement && target.isContentEditable)
    );
  }

  async function handleKeydown(event: KeyboardEvent) {
    const inInput = isEditableTarget(event.target);

    // Command+Left Arrow to go back
    if (event.key === 'ArrowLeft' && event.metaKey && !inInput) {
      event.preventDefault();
      event.stopPropagation();
      await closeDiffModal();
      return;
    }
    // Command+Up/Down Arrow to navigate between files
    if ((event.key === 'ArrowUp' || event.key === 'ArrowDown') && event.metaKey && !inInput) {
      event.preventDefault();
      event.stopPropagation();
      const currentPath = diffViewer.state.selectedFile;
      const idx = orderedFiles.findIndex((f) => f.path === currentPath);
      if (event.key === 'ArrowUp' && idx > 0) {
        await selectFile(orderedFiles[idx - 1]);
      } else if (event.key === 'ArrowDown' && idx < orderedFiles.length - 1) {
        await selectFile(orderedFiles[idx + 1]);
      }
      return;
    }
    // Escape to dismiss layers, then close modal (skip if focused on an editable element)
    if (event.key === 'Escape') {
      if (inInput) return;
      // If DiffViewer already handled this Escape (e.g. clearing selection/comment state),
      // don't also close the modal. Both handlers are on `document`, so stopPropagation
      // doesn't help — check defaultPrevented instead.
      if (event.defaultPrevented) return;
      // Close context dropdown first if open
      if (showContextDropdown) {
        event.preventDefault();
        event.stopPropagation();
        closeDropdown();
        return;
      }
      // Close the mobile file dialog before dismissing the whole diff.
      if (showMobileSidebar) {
        event.preventDefault();
        event.stopPropagation();
        showMobileSidebar = false;
        return;
      }
      // Close search bar first if open
      if (searchState.state.isOpen) {
        event.preventDefault();
        event.stopPropagation();
        searchState.closeSearch();
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      await closeDiffModal();
      return;
    }
    // Hold A to reveal AI annotations
    if (event.key === 'a' || event.key === 'A') {
      if (inInput) return;
      if (!event.repeat) {
        annotationsRevealed = true;
      }
    }
  }

  function handleKeyup(event: KeyboardEvent) {
    if (event.key === 'a' || event.key === 'A') {
      if (isEditableTarget(event.target)) return;
      annotationsRevealed = false;
    }
  }

  // Initialize collapsed state when search results are ready (only once per search)
  $effect(() => {
    checkSearchInitialization();
  });

  // Scope the diff theme's CSS variables onto the diff-viewer container so the
  // diff area reflects the chosen diff theme while the app chrome stays on its
  // fixed light/dark mode.
  $effect(() => {
    const el = diffViewerContainerEl;
    if (!el) return;
    for (const [prop, value] of Object.entries(preferences.diffThemeVars)) {
      el.style.setProperty(prop, value);
    }
  });

  // Set up keyboard navigation for diff viewer and search
  $effect(() => {
    // Create search navigation handlers
    const { onNextSearchResult, onPrevSearchResult } = createSearchNavigationHandlers({
      searchState,
      selectFile: selectFilePath,
      getFiles: () => diffViewer.state.files,
      onJumpToLine: (lineIndex: number) => {
        lineJumpToken += 1;
        jumpToLine = { lineIndex, token: lineJumpToken };
      },
    });

    const cleanup = setupDiffKeyboardNav({
      onOpenSearch: () => searchState.openSearch(),
      onNextSearchResult,
      onPrevSearchResult,
    });

    return cleanup;
  });

  onMount(() => {
    const mediaQuery = window.matchMedia(`(max-width: ${SMALL_DIFF_BREAKPOINT}px)`);
    const updateSmallDiffViewport = () => {
      isSmallDiffViewport = mediaQuery.matches;
      if (!isSmallDiffViewport) {
        showMobileSidebar = false;
        resetMobileDiffDrag();
      }
    };

    updateSmallDiffViewport();
    mediaQuery.addEventListener('change', updateSmallDiffViewport);
    document.addEventListener('keydown', handleKeydown);
    document.addEventListener('keyup', handleKeyup);
    document.addEventListener('click', handleClickOutsideDropdown);
    return () => {
      mediaQuery.removeEventListener('change', updateSmallDiffViewport);
      document.removeEventListener('keydown', handleKeydown);
      document.removeEventListener('keyup', handleKeyup);
      document.removeEventListener('click', handleClickOutsideDropdown);
    };
  });

  function getMobileDiffMaxDrag(): number {
    const width = diffViewerContainerEl?.clientWidth ?? 0;
    return Math.max(0, width - MOBILE_DIFF_EDGE_PEEK - MOBILE_DIFF_REST_OFFSET);
  }

  function resetMobileDiffDrag() {
    mobileDiffPointerId = null;
    mobileDiffIsDragging = false;
    mobileDiffGestureMode = 'pending';
    mobileDiffCanDragPane = false;
    mobileDiffCanScrollCode = false;
    mobileDiffMarkdownScrollEl = null;
    mobileDiffDragX = 0;
  }

  function isMobileDiffInteractiveTarget(target: HTMLElement): boolean {
    return !!target.closest(
      'button, a, input, textarea, select, [role="button"], [contenteditable="true"], .scrollbar, .scrollbar-h, .range-toolbar, .line-selection-toolbar, .comment-editor'
    );
  }

  function isMobileDiffCodeTarget(target: HTMLElement): boolean {
    return !!target.closest(
      '.after-pane .code-container, .after-pane .lines-wrapper, .after-pane .line'
    );
  }

  function getMobileDiffMarkdownScrollTarget(target: HTMLElement): HTMLElement | null {
    return target.closest('.after-pane .code-area.markdown-mode') as HTMLElement | null;
  }

  function canScrollMobileDiffContent(): boolean {
    return mobileDiffCanScrollCode || mobileDiffMarkdownScrollEl !== null;
  }

  function scrollMobileDiffContent(deltaY: number) {
    if (mobileDiffMarkdownScrollEl) {
      mobileDiffMarkdownScrollEl.scrollTop += deltaY;
      return;
    }

    diffViewerScrollApi?.scrollBy('after', deltaY);
  }

  function isMobileDiffRevealTarget(
    target: HTMLElement,
    event: PointerEvent | MouseEvent
  ): boolean {
    if (target.closest('.spine')) return true;

    const afterPane = target.closest('.after-pane') as HTMLElement | null;
    if (!afterPane) return false;

    const paneRect = afterPane.getBoundingClientRect();
    return event.clientX - paneRect.left <= MOBILE_DIFF_REVEAL_HIT_WIDTH;
  }

  function handleMobileDiffPointerDown(event: PointerEvent) {
    if (!isSmallDiffViewport) return;
    if (event.pointerType === 'mouse' && event.button !== 0) return;

    const target = event.target as HTMLElement;
    const canDragPane = isMobileDiffRevealTarget(target, event);
    const markdownScrollEl = !canDragPane ? getMobileDiffMarkdownScrollTarget(target) : null;
    const canScrollCode =
      !canDragPane && markdownScrollEl === null && isMobileDiffCodeTarget(target);
    if (!canDragPane && !canScrollCode && markdownScrollEl === null) return;
    const isMarkdownLink = markdownScrollEl !== null && target.closest('a') !== null;
    if (isMobileDiffInteractiveTarget(target) && !isMarkdownLink) return;

    mobileDiffPointerId = event.pointerId;
    mobileDiffStartX = event.clientX;
    mobileDiffStartY = event.clientY;
    mobileDiffLastX = event.clientX;
    mobileDiffLastY = event.clientY;
    mobileDiffStartDragX = mobileDiffDragX;
    mobileDiffIsDragging = false;
    mobileDiffGestureMode = 'pending';
    mobileDiffCanDragPane = canDragPane;
    mobileDiffCanScrollCode = canScrollCode;
    mobileDiffMarkdownScrollEl = markdownScrollEl;
  }

  function handleMobileDiffPointerMove(event: PointerEvent) {
    if (!isSmallDiffViewport || mobileDiffPointerId !== event.pointerId) return;

    const deltaX = event.clientX - mobileDiffStartX;
    const deltaY = event.clientY - mobileDiffStartY;

    if (mobileDiffGestureMode === 'vertical-scroll') {
      event.preventDefault();
      const moveDeltaY = event.clientY - mobileDiffLastY;
      mobileDiffLastY = event.clientY;
      scrollMobileDiffContent(-moveDeltaY);
      return;
    }

    if (mobileDiffGestureMode === 'horizontal-scroll') {
      event.preventDefault();
      const moveDeltaX = event.clientX - mobileDiffLastX;
      mobileDiffLastX = event.clientX;
      diffViewerScrollApi?.scrollByXBoth(-moveDeltaX);
      return;
    }

    if (mobileDiffGestureMode === 'pane-drag') {
      event.preventDefault();
      const nextX = mobileDiffStartDragX + deltaX;
      mobileDiffDragX = Math.max(0, Math.min(getMobileDiffMaxDrag(), nextX));
      return;
    }

    // Determine gesture direction once past the dead zone, then keep that mode
    // for the rest of the pointer sequence.
    if (Math.abs(deltaX) > 8 && Math.abs(deltaX) > Math.abs(deltaY)) {
      if (mobileDiffCanDragPane) {
        mobileDiffGestureMode = 'pane-drag';
        mobileDiffIsDragging = true;
        diffViewerContainerEl?.setPointerCapture(event.pointerId);
      } else if (mobileDiffCanScrollCode && diffViewerScrollApi?.canScrollX('after')) {
        mobileDiffGestureMode = 'horizontal-scroll';
        mobileDiffLastX = event.clientX;
        diffViewerContainerEl?.setPointerCapture(event.pointerId);
        event.preventDefault();
        diffViewerScrollApi.scrollByXBoth(-deltaX);
        return;
      } else {
        return;
      }
    } else if (
      canScrollMobileDiffContent() &&
      Math.abs(deltaY) > 8 &&
      Math.abs(deltaY) > Math.abs(deltaX)
    ) {
      mobileDiffGestureMode = 'vertical-scroll';
      mobileDiffLastY = event.clientY;
      event.preventDefault();
      scrollMobileDiffContent(-deltaY);
      return;
    } else {
      return;
    }

    event.preventDefault();
    const nextX = mobileDiffStartDragX + deltaX;
    mobileDiffDragX = Math.max(0, Math.min(getMobileDiffMaxDrag(), nextX));
  }

  function handleMobileDiffPointerUp(event: PointerEvent) {
    if (mobileDiffPointerId !== event.pointerId) return;
    if (diffViewerContainerEl?.hasPointerCapture(event.pointerId)) {
      diffViewerContainerEl.releasePointerCapture(event.pointerId);
    }
    resetMobileDiffDrag();
  }

  function handleMobileDiffMouseDownCapture(event: MouseEvent) {
    if (!isSmallDiffViewport) return;
    const target = event.target as HTMLElement;
    if (!isMobileDiffRevealTarget(target, event)) return;
    event.stopPropagation();
  }
</script>

{#snippet fileSidebarContents()}
  {#if diffViewer.state.loading}
    <div class="sidebar-loading">
      <Spinner size={14} />
      <span>Loading files...</span>
    </div>
  {:else if diffViewer.state.error}
    <div class="sidebar-error">
      <span>{diffViewer.state.error}</span>
    </div>
  {:else if diffViewer.state.files.length === 0}
    <div class="sidebar-empty">
      <span>No changes</span>
    </div>
  {:else}
    <div class="sidebar-content">
      <div class="sidebar-scroll">
        <!-- Search bar -->
        <CrossFileSearchBar
          files={diffViewer.state.files}
          loadFileDiff={loadFileDiffForSearch}
          {searchState}
        />

        <DiffFileTreeSection
          {readonly}
          {fileEntries}
          {needsReview}
          {reviewed}
          {readonlyTree}
          {needsReviewTree}
          {reviewedTree}
          selectedFile={diffViewer.state.selectedFile}
          {isCollapsed}
          onToggleDir={toggleDir}
          onSelectFile={selectFile}
          onToggleReviewed={toggleReviewed}
          onJumpToLine={handleJumpToLine}
          {searchState}
          diffViewerState={diffViewerForFileTree}
        />
        {#if !readonly}
          <DiffReferenceSection
            referenceFiles={reviewHandle?.state.referenceFiles ?? []}
            selectedFile={diffViewer.state.selectedFile}
            onSelectFile={selectFilePath}
            onRemoveReferenceFile={handleRemoveReferenceFile}
          />

          <DiffCommentsSection
            comments={currentComments}
            deletedComments={reviewHandle?.state.deletedComments ?? []}
            {selectedCommentId}
            {copiedFeedback}
            onSelectComment={handleSelectComment}
            onCopyAll={handleCopyComments}
            onDeleteAll={handleDeleteAllComments}
            onDeleteComment={handleDeleteComment}
            onRestoreComment={handleRestoreComment}
            commentNoteState={getCommentNoteState}
            commentCommitState={getCommentCommitState}
          />
        {/if}
      </div>

      {#if !readonly && diffViewer.state.commitSha}
        <DiffCommitSessionLauncher
          {branchId}
          {projectId}
          commitSha={diffViewer.state.commitSha}
          scope={reviewableScope()}
          reviewId={activeReviewId}
          visibleCommentCount={currentComments.length}
          {githubRepo}
          {subpath}
          {isRemote}
          onBeforeStart={prepareCommitSessionStart}
          onRequestClose={onClose}
        />
      {/if}
    </div>
  {/if}
{/snippet}

<TopBarPortal
  title={projectName ?? 'Diff'}
  badges={diffTopBarBadges}
  rightActions={diffTopBarActions}
/>

{#snippet diffTopBarBadges()}
  {#if githubRepo}
    <RepoLabel {githubRepo} {subpath} />
  {/if}
{/snippet}

{#snippet diffTopBarActions()}
  {#if showContextSwitcher}
    <div class="context-switcher">
      <span class="inline-flex" title="Switch diff context">
        <Button
          type="button"
          variant="outline"
          class="h-auto gap-1.5 rounded-md border-[var(--border-subtle)] bg-transparent px-2.5 py-1 text-xs font-medium text-muted-foreground shadow-none hover:border-[var(--border-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-3"
          disabled={switchingContext}
          onclick={() => (showContextDropdown ? closeDropdown() : openDropdown())}
          onkeydown={handleDropdownKeydown}
          aria-expanded={showContextDropdown}
          aria-haspopup="listbox"
        >
          {#if activeScope === 'branch'}
            <GitBranch size={12} />
          {:else}
            <GitCommitHorizontal size={12} />
          {/if}
          <span class="context-label">{contextLabel}</span>
          <ChevronDown size={12} />
        </Button>
      </span>
      {#if showContextDropdown}
        <div class="context-dropdown" role="listbox" aria-label="Diff context">
          {#each reversedCommits as commit, i (commit.sha)}
            <button
              type="button"
              class="context-option commit-option"
              role="option"
              aria-selected={activeScope === 'commit' && activeCommitSha === commit.sha}
              class:selected={activeScope === 'commit' && activeCommitSha === commit.sha}
              class:focused={dropdownFocusIndex === i}
              onclick={() => switchDiffContext('commit', commit.sha)}
            >
              <span class="commit-icon-wrapper"><GitCommitHorizontal size={14} /></span>
              <span class="option-label commit-option-label">
                <span class="commit-subject">{commit.subject}</span>
                <span class="commit-meta">
                  <span class="commit-sha">{commit.shortSha}</span>
                  <span class="commit-time">{formatRelativeTimeSeconds(commit.timestamp)}</span>
                </span>
              </span>
            </button>
          {/each}
          <div class="context-separator"></div>
          <button
            type="button"
            class="context-option"
            role="option"
            aria-selected={activeScope === 'branch'}
            class:selected={activeScope === 'branch'}
            class:focused={dropdownFocusIndex === reversedCommits.length}
            onclick={() => switchDiffContext('branch')}
          >
            <GitBranch size={14} />
            <span class="option-label">All changes</span>
          </button>
        </div>
      {/if}
    </div>
  {/if}

  {#if isSmallDiffViewport}
    <Button
      variant="ghost"
      size="icon-xs"
      class="max-md:size-10 [&_svg]:size-3.5"
      title="Show changed files"
      aria-label="Show changed files"
      onclick={() => (showMobileSidebar = true)}
    >
      <PanelRightOpen size={14} />
    </Button>
  {/if}
{/snippet}

{#snippet reviewCommentActions(context: CommentActionContext)}
  <ReviewCommentActions
    comment={context.comment}
    ensureSaved={context.ensureSaved}
    noteState={context.comment ? getCommentNoteState(context.comment) : 'idle'}
    commitState={context.comment ? getCommentCommitState(context.comment) : 'idle'}
    githubState={context.comment ? getCommentGithubState(context.comment) : 'idle'}
    {hasPr}
    onNote={handleNewNote}
    onCommit={handleNewCommit}
    onGithub={handleSendToGithub}
  />
{/snippet}

<div class="diff-detail" bind:this={diffDetailEl}>
  <div class="modal-body">
    <!-- Diff viewer -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="diff-viewer-container"
      class:mobile-diff-dragging={mobileDiffIsDragging}
      style={mobileDiffStyle}
      bind:this={diffViewerContainerEl}
      onpointerdown={handleMobileDiffPointerDown}
      onpointermove={handleMobileDiffPointerMove}
      onpointerup={handleMobileDiffPointerUp}
      onpointercancel={handleMobileDiffPointerUp}
      onmousedowncapture={handleMobileDiffMouseDownCapture}
    >
      <DiffViewer
        bind:this={diffViewerComponent}
        diff={currentDiff}
        comments={readonly ? [] : currentComments}
        {jumpToComment}
        {jumpToLine}
        loading={diffViewer.state.loadingFile !== null && !diffViewer.state.loading}
        emptyMessage={diffViewerEmptyMessage}
        beforeLabel={activeBeforeLabel}
        afterLabel={activeAfterLabel}
        annotations={revealedAnnotations}
        {annotationsRevealed}
        searchState={searchState.state}
        syntaxThemeVersion={preferences.diffThemeVersion}
        onAddComment={readonly ? undefined : handleAddComment}
        onUpdateComment={readonly ? undefined : handleUpdateComment}
        onDeleteComment={readonly ? undefined : handleDeleteCommentFromViewer}
        commentActions={readonly ? undefined : reviewCommentActions}
        clickDismissBoundary={diffDetailEl}
        bind:scrollApi={diffViewerScrollApi}
      />
    </div>

    <!-- File sidebar (right side) -->
    {#if !isSmallDiffViewport}
      <div class="file-sidebar">
        {@render fileSidebarContents()}
      </div>
    {/if}
  </div>

  {#if isSmallDiffViewport && showMobileSidebar}
    <div class="mobile-sidebar-backdrop" role="dialog" aria-modal="true" aria-label="Changed files">
      <div class="mobile-sidebar-dialog">
        <div class="mobile-sidebar-header">
          <span class="mobile-sidebar-title">Changed files</span>
          <Button
            variant="ghost"
            size="icon"
            class="size-auto shrink-0 rounded-md p-[5px] text-muted-foreground shadow-none hover:bg-[var(--bg-hover)] hover:text-foreground disabled:opacity-35 [&_svg]:!size-3.5"
            title="Close changed files"
            aria-label="Close changed files"
            onclick={() => (showMobileSidebar = false)}
          >
            <X size={14} />
          </Button>
        </div>
        <div class="mobile-sidebar-body">
          {@render fileSidebarContents()}
        </div>
      </div>
    </div>
  {/if}

  {#if showNewSessionModal && branch}
    <NewSessionModal
      open={true}
      {branch}
      mode={newSessionMode}
      initialPrompt={newSessionPrefill}
      prefilled
      prefillSelection="last-line"
      onClose={() => {
        showNewSessionModal = false;
        newSessionComment = null;
      }}
      onSubmit={handleSessionModalSubmit}
    />
  {/if}

  <SessionModal
    open={openSessionId !== null}
    sessionId={openSessionId ?? ''}
    repoDir={branch?.worktreePath}
    {branchId}
    {projectId}
    repoLabel={githubRepo ? { githubRepo, subpath: subpath ?? null, headRepo: null } : null}
    referenceNav={disabledReferenceNav}
    onClose={() => {
      openSessionId = null;
    }}
    onHashtagClick={handleHashtagClick}
  />

  <NoteModal
    open={openNote !== null}
    title={openNote?.title ?? ''}
    content={openNote?.content ?? ''}
    sessionId={openNote?.sessionId}
    noteUpdatedAt={openNote?.noteUpdatedAt}
    noteId={openNote?.noteId}
    noteKind="branch"
    {branchId}
    {projectId}
    repoDir={branch?.worktreePath}
    repoLabel={githubRepo ? { githubRepo, subpath: subpath ?? null, headRepo: null } : null}
    chatOpen={openNote?.chatOpen ?? false}
    onChatOpenChange={(chatOpen) => {
      if (openNote) openNote = { ...openNote, chatOpen };
    }}
    {hashtagItems}
    referenceNav={disabledReferenceNav}
    onClose={() => {
      openNote = null;
    }}
    onHashtagClick={handleHashtagClick}
  />
</div>

<style>
  .diff-detail {
    display: flex;
    flex-direction: column;
    width: 100%;
    flex: 1;
    min-height: 0;
    background-color: var(--bg-chrome);
    overflow: hidden;
  }

  /* ========================================================================
   * Context switcher
   * ====================================================================== */

  .context-switcher {
    position: relative;
    -webkit-app-region: no-drag;
  }

  .context-label {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 200px;
  }

  .context-dropdown {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    background: var(--bg-menu);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    box-shadow: var(--shadow-elevated);
    overflow: hidden;
    z-index: var(--z-index-floating);
    min-width: 220px;
    max-width: 340px;
    max-height: 320px;
    overflow-y: auto;
  }

  .context-separator {
    height: 1px;
    background: var(--border-subtle);
    margin: 2px 0;
  }

  .context-option {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 7px 12px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .context-option:hover,
  .context-option.focused {
    background-color: var(--bg-hover);
  }

  .context-option.selected {
    color: var(--ui-accent);
  }

  .context-option.selected :global(svg) {
    color: var(--ui-accent);
  }

  .context-option .option-label {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    overflow: hidden;
  }

  .commit-option {
    align-items: flex-start;
  }

  .commit-icon-wrapper {
    display: flex;
    margin-top: 2px;
  }

  .commit-option-label {
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
  }

  .commit-subject {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    width: 100%;
  }

  .commit-meta {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .commit-sha {
    font-family: var(--font-mono, monospace);
  }

  .commit-time {
    white-space: nowrap;
  }

  /* ========================================================================
   * Body layout
   * ====================================================================== */

  .modal-body {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  /* ========================================================================
   * File sidebar
   * ====================================================================== */

  .file-sidebar {
    width: 240px;
    flex-shrink: 0;
    border-left: none;
    overflow: hidden;
  }

  .sidebar-content {
    display: flex;
    flex-direction: column;
    height: 100%;
    padding: 0;
  }

  .sidebar-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
  }

  .sidebar-loading,
  .sidebar-error,
  .sidebar-empty {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .sidebar-error {
    color: var(--ui-danger);
  }

  /* ========================================================================
   * Diff viewer container
   * ====================================================================== */

  .diff-viewer-container {
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }

  .mobile-sidebar-backdrop {
    position: fixed;
    inset: 0;
    z-index: 1002;
    background: var(--bg-chrome);
  }

  .mobile-sidebar-dialog {
    display: flex;
    flex-direction: column;
    width: 100vw;
    height: 100vh;
    height: 100dvh;
    background: var(--bg-chrome);
  }

  .mobile-sidebar-header {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 44px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border-subtle);
    flex-shrink: 0;
  }

  .mobile-sidebar-title {
    flex: 1;
    min-width: 0;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 600;
  }

  .mobile-sidebar-body {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  :global(.diff-viewer-container .comment-editor) {
    background: var(--diff-comment-bg);
    border-color: var(--diff-comment-border);
    box-shadow:
      0 0 0 1px color-mix(in srgb, var(--diff-comment-accent) 18%, transparent),
      0 0 0 3px color-mix(in srgb, var(--diff-comment-accent) 8%, transparent),
      var(--shadow-elevated);
  }

  :global(.diff-viewer-container .comment-editor-hint) {
    background-color: color-mix(in srgb, var(--diff-comment-bg-emphasis) 88%, var(--bg-chrome));
    border-top: 1px solid color-mix(in srgb, var(--border-muted) 88%, transparent);
  }

  :global(.diff-viewer-container .comment-textarea::placeholder) {
    color: color-mix(in srgb, var(--diff-comment-accent) 55%, var(--text-muted));
  }

  :global(.diff-viewer-container .range-toolbar),
  :global(.diff-viewer-container .line-selection-toolbar) {
    background-color: var(--diff-comment-bg);
    border: 1px solid color-mix(in srgb, var(--diff-comment-border) 88%, transparent);
    box-shadow:
      0 0 0 1px color-mix(in srgb, var(--diff-comment-accent) 10%, transparent),
      var(--shadow-elevated);
  }

  :global(.diff-viewer-container .range-btn.comment-btn),
  :global(.diff-viewer-container .selection-info) {
    color: var(--diff-comment-accent);
  }

  :global(.diff-viewer-container .range-btn.comment-btn:hover) {
    color: var(--text-primary);
    background-color: color-mix(in srgb, var(--diff-comment-accent) 14%, var(--diff-comment-bg));
  }

  @media (max-width: 700px) {
    .context-label {
      max-width: 96px;
    }

    .modal-body {
      position: relative;
    }

    .diff-viewer-container {
      --mobile-diff-spine-width: 16px;
      width: 100%;
    }

    :global(.diff-viewer-container .diff-content) {
      padding-left: 0;
    }

    :global(.diff-viewer-container .diff-content:not(.single-pane)) {
      position: relative;
      isolation: isolate;
    }

    :global(.diff-viewer-container .diff-content:not(.single-pane) .before-pane) {
      position: absolute;
      inset: 0 auto 0 0;
      width: calc(100% - var(--mobile-diff-edge-peek));
      flex: none !important;
      border-radius: 0;
      z-index: 1;
    }

    :global(.diff-viewer-container .diff-content:not(.single-pane) .after-pane) {
      position: absolute;
      inset: 0 auto 0 0;
      width: calc(100% - var(--mobile-diff-rest-offset));
      flex: none !important;
      border-radius: 0;
      transform: translateX(calc(var(--mobile-diff-rest-offset) + var(--mobile-diff-drag-x, 0px)));
      transition: transform 0.22s ease;
      z-index: 3;
      cursor: default;
      touch-action: none;
      box-shadow:
        -18px 0 28px color-mix(in srgb, var(--bg-deepest) 52%, transparent),
        var(--shadow-elevated);
    }

    :global(.diff-viewer-container .diff-content:not(.single-pane) .after-pane .pane-header) {
      padding-left: calc(var(--mobile-diff-spine-width) + 12px);
    }

    :global(.diff-viewer-container .diff-content:not(.single-pane) .after-pane .code-area) {
      margin-left: var(--mobile-diff-spine-width);
    }

    :global(.diff-viewer-container.mobile-diff-dragging .diff-content .after-pane) {
      cursor: grabbing;
      transition: none;
      user-select: none;
    }

    :global(.diff-viewer-container .diff-content:not(.single-pane) .spine) {
      position: absolute;
      inset: 0 auto 0 0;
      width: var(--mobile-diff-spine-width);
      flex: none !important;
      transform: translateX(calc(var(--mobile-diff-rest-offset) + var(--mobile-diff-drag-x, 0px)));
      transition: transform 0.22s ease;
      z-index: 4;
      cursor: grab;
      touch-action: none;
      background-color: var(--bg-primary);
    }

    :global(.diff-viewer-container.mobile-diff-dragging .diff-content .spine) {
      cursor: grabbing;
      transition: none;
      user-select: none;
    }

    :global(.diff-viewer-container .diff-content:not(.single-pane) .spine .divider-handle) {
      display: none;
    }
  }
</style>
