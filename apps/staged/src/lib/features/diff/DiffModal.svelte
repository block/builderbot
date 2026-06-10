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
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import GitCommitHorizontal from '@lucide/svelte/icons/git-commit-horizontal';
  import PanelRightOpen from '@lucide/svelte/icons/panel-right-open';
  import { getWindowSync } from '../../transport';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { Button } from '$lib/components/ui/button';
  import * as Tooltip from '$lib/components/ui/tooltip';
  import { toast } from 'svelte-sonner';
  import { formatRelativeTimeSeconds } from '../../shared/relativeTime.svelte';
  import { DiffViewer, CrossFileSearchBar } from '@builderbot/diff-viewer/components';
  import DiffCommentsSection from './DiffCommentsSection.svelte';
  import DiffFileTreeSection from './DiffFileTreeSection.svelte';
  import DiffCommitSessionLauncher from './DiffCommitSessionLauncher.svelte';
  import DiffReferenceSection from './DiffReferenceSection.svelte';
  import NewSessionModal from '../sessions/NewSessionModal.svelte';
  import { getPreferredAgent, preferences } from '../settings/preferences.svelte';
  import { agentState, REMOTE_AGENTS } from '../agents/agent.svelte';
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
    Branch,
    BranchSessionType,
    Comment,
    CommitTimelineItem,
    SmartDiffAnnotation,
    Span,
  } from '../../types';
  import type { DiffScope } from '../../commands';
  import { findFreshAutoReview, getSession } from '../../commands';
  import * as commands from '../../api/commands';
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
  import { viewport } from '../../shared/viewport.svelte';

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

  function handleDropdownKeydown(event: KeyboardEvent) {
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
        switchDiffContext('branch');
      } else if (dropdownFocusIndex >= 0 && dropdownFocusIndex < commitCount) {
        const commit = reversedCommits[dropdownFocusIndex];
        if (commit) switchDiffContext('commit', commit.sha);
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
  let diffViewerContainerEl: HTMLDivElement | null = $state(null);
  let mobileDiffDragX = $state(0);
  let mobileDiffPointerId: number | null = null;
  let mobileDiffStartX = 0;
  let mobileDiffStartY = 0;
  let mobileDiffStartDragX = 0;
  let mobileDiffIsDragging = $state(false);
  let mobileDiffIsScrolling = false;
  let mobileDiffLastY = 0;
  let diffViewerScrollApi: { scrollBy: (side: 'before' | 'after', deltaY: number) => void } | null =
    $state(null);
  let mobileDiffStyle = $derived(
    `--mobile-diff-drag-x: ${mobileDiffDragX}px;` +
      `--mobile-diff-rest-offset: ${MOBILE_DIFF_REST_OFFSET}px;` +
      `--mobile-diff-edge-peek: ${MOBILE_DIFF_EDGE_PEEK}px;`
  );

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
    stopAutoReviewPolling();
  });

  // ==========================================================================
  // Branch data (for PR info and session launching)
  // ==========================================================================

  let branch = $state<Branch | null>(null);

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

  // ==========================================================================
  // New Session Modal state
  // ==========================================================================

  let showNewSessionModal = $state(false);
  let newSessionMode = $state<BranchSessionType>('note');
  let newSessionPrefill = $state('');

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

    try {
      // Check if there's a running session to decide queue vs start
      const timeline = await commands.getBranchTimeline(branchId, { force: true });
      const hasRunning =
        timeline.commits.some(
          (c) => c.sessionStatus === 'running' || c.sessionStatus === 'queued'
        ) ||
        timeline.notes.some((n) => n.sessionStatus === 'running' || n.sessionStatus === 'queued') ||
        timeline.reviews.some(
          (r) => !r.isAuto && (r.sessionStatus === 'running' || r.sessionStatus === 'queued')
        );

      const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
      const provider = getPreferredAgent(agents) ?? undefined;

      if (hasRunning) {
        await commands.queueBranchSession(
          branchId,
          prompt,
          mode,
          provider,
          undefined,
          launchContext
        );
      } else {
        await commands.startBranchSession(
          branchId,
          prompt,
          mode,
          provider,
          undefined,
          launchContext
        );
      }

      const label = mode === 'note' ? 'Note' : 'Commit';
      toast.success(`${label} session ${hasRunning ? 'queued' : 'started'}`);
    } catch (e) {
      toast.error(`Unable to start ${mode} session`, {
        description: e instanceof Error ? e.message : String(e),
        duration: Infinity,
      });
    }
  }

  function handleNewNote(comment: Comment, event: MouseEvent) {
    if (event.altKey) {
      launchSessionDirectly(comment, 'note');
      return;
    }
    newSessionMode = 'note';
    newSessionPrefill = buildCommentPrompt(comment, 'note');
    showNewSessionModal = true;
  }

  function handleNewCommit(comment: Comment, event: MouseEvent) {
    if (event.altKey) {
      launchSessionDirectly(comment, 'commit');
      return;
    }
    newSessionMode = 'commit';
    newSessionPrefill = buildCommentPrompt(comment, 'commit');
    showNewSessionModal = true;
  }

  async function handleSessionModalSubmit(data: {
    prompt: string;
    mode: BranchSessionType;
    imageIds: string[];
  }) {
    showNewSessionModal = false;
    const launchContext = {
      source: 'diff_viewer' as const,
      scope: reviewableScope(),
      commitSha: diffViewer.state.commitSha ?? '',
      reviewId: activeReviewId ?? null,
    };

    try {
      const timeline = await commands.getBranchTimeline(branchId, { force: true });
      const hasRunning =
        timeline.commits.some(
          (c) => c.sessionStatus === 'running' || c.sessionStatus === 'queued'
        ) ||
        timeline.notes.some((n) => n.sessionStatus === 'running' || n.sessionStatus === 'queued') ||
        timeline.reviews.some(
          (r) => !r.isAuto && (r.sessionStatus === 'running' || r.sessionStatus === 'queued')
        );

      const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
      const provider = getPreferredAgent(agents) ?? undefined;

      if (hasRunning) {
        await commands.queueBranchSession(
          branchId,
          data.prompt,
          data.mode,
          provider,
          data.imageIds,
          launchContext
        );
      } else {
        await commands.startBranchSession(
          branchId,
          data.prompt,
          data.mode,
          provider,
          data.imageIds,
          launchContext
        );
      }

      const label = data.mode === 'note' ? 'Note' : data.mode === 'commit' ? 'Commit' : 'Review';
      toast.success(`${label} session ${hasRunning ? 'queued' : 'started'}`);
    } catch (e) {
      toast.error(`Unable to start ${data.mode} session`, {
        description: e instanceof Error ? e.message : String(e),
        duration: Infinity,
      });
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

  // Create tracker for search initialization
  const checkSearchInitialization = createSearchInitializationTracker({
    searchState,
    getFiles: () => diffViewer.state.files,
  });

  // ==========================================================================
  // Derived
  // ==========================================================================

  let currentDiff = $derived(diffViewer.getCurrentDiff());

  // Render-tail timing: `computeLineDiff` is sub-millisecond, but the syntax
  // highlighting, DOM construction, and layout/paint the DiffViewer performs
  // afterwards are never measured. Once a file's diff is in the cache and
  // selected, time how long it takes to actually hit the screen via a
  // double-rAF (the second frame fires after the browser has painted). Guarded
  // by path so unrelated reactive updates (comments, search) don't re-log.
  let renderTimedPath: string | null = null;
  $effect(() => {
    const path = diffViewer.state.selectedFile;
    const ready = currentDiff !== null && diffViewer.state.loadingFile === null;
    if (!path || !ready || renderTimedPath === path) return;
    renderTimedPath = path;
    const t0 = performance.now();
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        console.info(
          `[diff] render painted in ${Math.round(performance.now() - t0)}ms: path=${path}`
        );
      });
    });
  });
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

  function selectFile(file: FileEntry) {
    selectedCommentId = null;
    if (isSmallDiffViewport) showMobileSidebar = false;
    handleSearchOnFileSelect(file.path);
    diffViewer.selectFile(file.path);
  }

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

  async function handleAddComment(path: string, span: Span, content: string): Promise<void> {
    await reviewHandle?.addComment(path, span, content);
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

  function handleKeydown(event: KeyboardEvent) {
    const inInput = isEditableTarget(event.target);

    // Command+Left Arrow to go back
    if (event.key === 'ArrowLeft' && event.metaKey && !inInput) {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    // Command+Up/Down Arrow to navigate between files
    if ((event.key === 'ArrowUp' || event.key === 'ArrowDown') && event.metaKey && !inInput) {
      event.preventDefault();
      event.stopPropagation();
      const currentPath = diffViewer.state.selectedFile;
      const idx = orderedFiles.findIndex((f) => f.path === currentPath);
      if (event.key === 'ArrowUp' && idx > 0) {
        selectFile(orderedFiles[idx - 1]);
      } else if (event.key === 'ArrowDown' && idx < orderedFiles.length - 1) {
        selectFile(orderedFiles[idx + 1]);
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
      onClose();
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
      selectFile: (path: string) => diffViewer.selectFile(path),
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

  // ==========================================================================
  // Title bar drag support
  // ==========================================================================

  function startDrag(e: PointerEvent) {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    const isInteractive = target.closest('button, a, input, [role="button"]');
    if (!isInteractive) {
      e.preventDefault();
      getWindowSync().startDragging();
    }
  }

  function getMobileDiffMaxDrag(): number {
    const width = diffViewerContainerEl?.clientWidth ?? 0;
    return Math.max(0, width - MOBILE_DIFF_EDGE_PEEK - MOBILE_DIFF_REST_OFFSET);
  }

  function resetMobileDiffDrag() {
    mobileDiffPointerId = null;
    mobileDiffIsDragging = false;
    mobileDiffIsScrolling = false;
    mobileDiffDragX = 0;
  }

  function isMobileDiffInteractiveTarget(target: HTMLElement): boolean {
    return !!target.closest(
      'button, a, input, textarea, select, [role="button"], [contenteditable="true"], .scrollbar, .scrollbar-h, .range-toolbar, .line-selection-toolbar, .comment-editor'
    );
  }

  function handleMobileDiffPointerDown(event: PointerEvent) {
    if (!isSmallDiffViewport) return;
    if (event.pointerType === 'mouse' && event.button !== 0) return;

    const target = event.target as HTMLElement;
    if (!target.closest('.after-pane, .spine')) return;
    if (isMobileDiffInteractiveTarget(target)) return;

    mobileDiffPointerId = event.pointerId;
    mobileDiffStartX = event.clientX;
    mobileDiffStartY = event.clientY;
    mobileDiffLastY = event.clientY;
    mobileDiffStartDragX = mobileDiffDragX;
    mobileDiffIsDragging = false;
    mobileDiffIsScrolling = false;
  }

  function handleMobileDiffPointerMove(event: PointerEvent) {
    if (!isSmallDiffViewport || mobileDiffPointerId !== event.pointerId) return;

    const deltaX = event.clientX - mobileDiffStartX;
    const deltaY = event.clientY - mobileDiffStartY;

    // Already in vertical scroll mode — translate touch delta to scroll
    if (mobileDiffIsScrolling) {
      event.preventDefault();
      const moveDeltaY = event.clientY - mobileDiffLastY;
      mobileDiffLastY = event.clientY;
      diffViewerScrollApi?.scrollBy('after', -moveDeltaY);
      return;
    }

    if (!mobileDiffIsDragging) {
      // Determine gesture direction once past dead zone
      if (Math.abs(deltaX) > 8 && Math.abs(deltaX) > Math.abs(deltaY)) {
        // Horizontal intent — capture pointer and start drag
        mobileDiffIsDragging = true;
        diffViewerContainerEl?.setPointerCapture(event.pointerId);
      } else if (Math.abs(deltaY) > 8 && Math.abs(deltaY) > Math.abs(deltaX)) {
        // Vertical intent — start scroll mode
        mobileDiffIsScrolling = true;
        mobileDiffLastY = event.clientY;
        event.preventDefault();
        diffViewerScrollApi?.scrollBy('after', -deltaY);
        return;
      } else {
        return;
      }
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
    if (!target.closest('.spine')) return;
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
          diffViewerState={diffViewer}
        />
        {#if !readonly}
          <DiffReferenceSection
            referenceFiles={reviewHandle?.state.referenceFiles ?? []}
            selectedFile={diffViewer.state.selectedFile}
            onSelectFile={(path) => {
              selectedCommentId = null;
              if (isSmallDiffViewport) showMobileSidebar = false;
              diffViewer.selectFile(path);
            }}
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
          onStarted={onClose}
        />
      {/if}
    </div>
  {/if}
{/snippet}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="diff-modal-backdrop" onkeydown={handleKeydown}>
  <div class="diff-modal">
    <!-- Title bar -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="title-bar" onpointerdown={startDrag}>
      <div class="traffic-light-spacer"></div>
      <div class="left-actions">
        <Tooltip.Root>
          <Tooltip.Trigger>
            {#snippet child({ props })}
              <Button
                {...props}
                variant="ghost"
                size="icon"
                class="size-auto shrink-0 rounded-md p-[5px] text-muted-foreground shadow-none hover:bg-[var(--bg-hover)] hover:text-foreground disabled:opacity-35 [&_svg]:!size-3.5"
                onclick={onClose}
              >
                <ArrowLeft size={14} />
              </Button>
            {/snippet}
          </Tooltip.Trigger>
          <Tooltip.Content>{viewport.showShortcutHints ? 'Back (Esc)' : 'Back'}</Tooltip.Content>
        </Tooltip.Root>
      </div>
      <div class="title-content">
        {#if projectName}
          <span class="project-name">{projectName}</span>
        {/if}
        {#if githubRepo}
          <RepoLabel {githubRepo} {subpath} />
        {/if}
      </div>
      <div class="drag-spacer"></div>
      {#if showContextSwitcher}
        <div class="context-switcher">
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <span {...props} class="inline-flex">
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
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Switch diff context</Tooltip.Content>
          </Tooltip.Root>
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
        <Tooltip.Root>
          <Tooltip.Trigger>
            {#snippet child({ props })}
              <Button
                {...props}
                variant="ghost"
                size="icon"
                class="size-auto shrink-0 rounded-md p-[5px] text-muted-foreground shadow-none hover:bg-[var(--bg-hover)] hover:text-foreground disabled:opacity-35 [&_svg]:!size-3.5"
                onclick={() => (showMobileSidebar = true)}
                aria-label="Show changed files"
              >
                <PanelRightOpen size={14} />
              </Button>
            {/snippet}
          </Tooltip.Trigger>
          <Tooltip.Content>Show changed files</Tooltip.Content>
        </Tooltip.Root>
      {/if}
    </div>

    <div class="modal-body">
      <!-- Diff viewer -->
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
          onCommentNote={readonly ? undefined : handleNewNote}
          onCommentCommit={readonly ? undefined : handleNewCommit}
          onCommentGithub={readonly || !hasPr ? undefined : handleSendToGithub}
          commentGithubState={readonly || !hasPr ? undefined : getCommentGithubState}
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
  </div>

  {#if isSmallDiffViewport && showMobileSidebar}
    <div class="mobile-sidebar-backdrop" role="dialog" aria-modal="true" aria-label="Changed files">
      <div class="mobile-sidebar-dialog">
        <div class="mobile-sidebar-header">
          <span class="mobile-sidebar-title">Changed files</span>
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="ghost"
                  size="icon"
                  class="size-auto shrink-0 rounded-md p-[5px] text-muted-foreground shadow-none hover:bg-[var(--bg-hover)] hover:text-foreground disabled:opacity-35 [&_svg]:!size-3.5"
                  onclick={() => (showMobileSidebar = false)}
                  aria-label="Close changed files"
                >
                  <X size={14} />
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Close changed files</Tooltip.Content>
          </Tooltip.Root>
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
      }}
      onSubmit={handleSessionModalSubmit}
    />
  {/if}
</div>

<style>
  /* ========================================================================
   * Modal backdrop + frame
   * ====================================================================== */

  .diff-modal-backdrop {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background-color: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: stretch;
    justify-content: center;
  }

  .diff-modal {
    display: flex;
    flex-direction: column;
    position: relative;
    width: 100%;
    height: 100%;
    background-color: var(--bg-chrome);
    border-radius: 0;
    border: none;
    box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
    overflow: hidden;
  }

  /* ========================================================================
   * Title bar
   * ====================================================================== */

  .title-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px;
    background: var(--bg-chrome);
    flex-shrink: 0;
    border-bottom: 1px solid var(--border-subtle);
  }

  .traffic-light-spacer {
    width: 70px;
    flex-shrink: 0;
    align-self: stretch;
  }

  .left-actions {
    display: flex;
    align-items: center;
  }

  .title-content {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-sm);
  }

  .project-name {
    color: var(--text-primary);
    font-weight: 500;
  }

  .drag-spacer {
    flex: 1;
    align-self: stretch;
    min-width: 20px;
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
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    overflow: hidden;
    z-index: 1001;
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
    .title-bar {
      gap: 6px;
    }

    .traffic-light-spacer {
      width: 58px;
    }

    .title-content {
      min-width: 0;
    }

    .project-name {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }

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
      cursor: grab;
      touch-action: pan-y;
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
      touch-action: pan-y;
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
