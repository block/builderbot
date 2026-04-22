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
  import {
    X,
    ArrowLeft,
    ChevronDown,
    Check,
    GitBranch,
    GitCommitHorizontal,
    Info,
  } from 'lucide-svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { formatRelativeTimeSeconds } from '../../shared/relativeTime.svelte';
  import { DiffViewer, CrossFileSearchBar } from '@builderbot/diff-viewer/components';
  import DiffCommentsSection from './DiffCommentsSection.svelte';
  import DiffFileTreeSection from './DiffFileTreeSection.svelte';
  import DiffCommitSessionLauncher from './DiffCommitSessionLauncher.svelte';
  import DiffReferenceSection from './DiffReferenceSection.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import { createDiffViewerState } from './diffViewerState.svelte';
  import { createReviewState } from './reviewState.svelte';
  import { createSearchState } from '@builderbot/diff-viewer/state';
  import {
    setupDiffKeyboardNav,
    createSearchNavigationHandlers,
    createSearchInitializationTracker,
    createFileSelectionWithSearch,
  } from '@builderbot/diff-viewer/utils';
  import type { Comment, CommitTimelineItem, SmartDiffAnnotation, Span } from '../../types';
  import { findFreshAutoReview, getSession } from '../../commands';
  import {
    buildFileEntries,
    buildTree,
    compactTree,
    formatLineRange,
    pathsMatch,
    truncateText,
    type FileEntry,
    type TreeNode,
  } from './diffModalHelpers';

  // ==========================================================================
  // Props
  // ==========================================================================

  interface Props {
    branchId: string;
    /** Project ID for scoping hashtag references to include project-level notes. */
    projectId?: string | null;
    /** Optional — for branch scope, resolved automatically. */
    commitSha?: string;
    scope?: 'branch' | 'commit';
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
    if (sha && !reviewHandle && !readonly) {
      // svelte-ignore state_referenced_locally
      reviewHandle = createReviewState(branchId, sha, activeScope, activeReviewId);
    }
  });

  // Context switcher state
  // svelte-ignore state_referenced_locally
  let activeScope = $state<'branch' | 'commit'>(scope);
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

  /** Label for the current context shown in the dropdown trigger. */
  let contextLabel = $derived(
    activeScope === 'branch'
      ? 'All changes'
      : (commits?.find((c) => c.sha === activeCommitSha)?.subject ?? 'Commit')
  );

  /** Whether the context switcher should be shown. */
  let showContextSwitcher = $derived((commits?.length ?? 0) > 0);

  /** The SHA of the latest (most recent) commit on the branch.
   *  Explicitly sorts by descending order to avoid depending on array ordering from the backend. */
  let latestCommitSha = $derived(
    commits?.filter((c) => c.sha).sort((a, b) => b.order - a.order)[0]?.sha
  );

  /** Whether the current context allows commenting and commit actions.
   *  True for branch scope ("All changes") and the latest commit; false for older commits. */
  let isContextEditable = $derived(
    activeScope === 'branch' ||
      (activeScope === 'commit' && activeCommitSha != null && activeCommitSha === latestCommitSha)
  );

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

  /** Index of the focused option in the dropdown (-1 = none). 0 = "All changes", 1+ = commits. */
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

  // Confirmation dialog state
  let commentToDelete = $state<string | null>(null);
  let showDeleteAllConfirm = $state(false);

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

  // Create tracker for search initialization
  const checkSearchInitialization = createSearchInitializationTracker({
    searchState,
    getFiles: () => diffViewer.state.files,
  });

  // ==========================================================================
  // Derived
  // ==========================================================================

  let currentDiff = $derived(diffViewer.getCurrentDiff());
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

  function handleDeleteComment(commentId: string) {
    commentToDelete = commentId;
  }

  async function confirmDeleteComment() {
    if (commentToDelete) {
      await reviewHandle?.deleteComment(commentToDelete);
      commentToDelete = null;
    }
  }

  function handleDeleteAllComments() {
    showDeleteAllConfirm = true;
  }

  async function confirmDeleteAllComments() {
    await reviewHandle?.deleteAllComments();
    showDeleteAllConfirm = false;
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
    document.addEventListener('keydown', handleKeydown);
    document.addEventListener('keyup', handleKeyup);
    document.addEventListener('click', handleClickOutsideDropdown);
    return () => {
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
      getCurrentWindow().startDragging();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="diff-modal-backdrop" onkeydown={handleKeydown}>
  <div class="diff-modal">
    <!-- Title bar -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="title-bar" onpointerdown={startDrag}>
      <div class="traffic-light-spacer"></div>
      <div class="left-actions">
        <button class="icon-btn" onclick={onClose} title="Back (Esc)">
          <ArrowLeft size={14} />
        </button>
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
          <button
            type="button"
            class="context-switcher-btn"
            disabled={switchingContext}
            onclick={() => (showContextDropdown ? closeDropdown() : openDropdown())}
            onkeydown={handleDropdownKeydown}
            aria-expanded={showContextDropdown}
            aria-haspopup="listbox"
            title="Switch diff context"
          >
            {#if activeScope === 'branch'}
              <GitBranch size={12} />
            {:else}
              <GitCommitHorizontal size={12} />
            {/if}
            <span class="context-label">{contextLabel}</span>
            <ChevronDown size={12} />
          </button>
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
                  {#if activeScope === 'commit' && activeCommitSha === commit.sha}
                    <Check size={14} />
                  {/if}
                </button>
              {/each}
              <div class="context-separator"></div>
              <button
                type="button"
                class="context-option"
                role="option"
                aria-selected={activeScope === 'branch'}
                class:selected={activeScope === 'branch'}
                class:focused={dropdownFocusIndex === (commits?.length ?? 0)}
                onclick={() => switchDiffContext('branch')}
              >
                <GitBranch size={14} />
                <span class="option-label">All changes</span>
                {#if activeScope === 'branch'}
                  <Check size={14} />
                {/if}
              </button>
            </div>
          {/if}
        </div>
      {/if}
    </div>

    <div class="modal-body">
      <!-- Diff viewer -->
      <div class="diff-viewer-container">
        <DiffViewer
          diff={currentDiff}
          comments={readonly ? [] : currentComments}
          {jumpToComment}
          {jumpToLine}
          loading={diffViewer.state.loadingFile !== null}
          beforeLabel={activeBeforeLabel}
          afterLabel={activeAfterLabel}
          annotations={revealedAnnotations}
          {annotationsRevealed}
          searchState={searchState.state}
          onAddComment={readonly || !isContextEditable ? undefined : handleAddComment}
          onUpdateComment={readonly || !isContextEditable ? undefined : handleUpdateComment}
          onDeleteComment={readonly || !isContextEditable
            ? undefined
            : handleDeleteCommentFromViewer}
        />
      </div>

      <!-- File sidebar (right side) -->
      <div class="file-sidebar">
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
              {#if !readonly && isContextEditable}
                <DiffReferenceSection
                  referenceFiles={reviewHandle?.state.referenceFiles ?? []}
                  selectedFile={diffViewer.state.selectedFile}
                  onSelectFile={(path) => {
                    selectedCommentId = null;
                    diffViewer.selectFile(path);
                  }}
                  onRemoveReferenceFile={handleRemoveReferenceFile}
                />

                <DiffCommentsSection
                  comments={currentComments}
                  {selectedCommentId}
                  {copiedFeedback}
                  onSelectComment={handleSelectComment}
                  onCopyAll={handleCopyComments}
                  onDeleteAll={handleDeleteAllComments}
                  onDeleteComment={handleDeleteComment}
                />
              {:else if !readonly && !isContextEditable}
                <div class="context-info-box">
                  <Info size={14} />
                  <span
                    >Comments and actions are only available when viewing
                    <button class="context-link-btn" onclick={() => switchDiffContext('branch')}
                      >all changes</button
                    >
                    or the
                    <button
                      class="context-link-btn"
                      onclick={() =>
                        latestCommitSha && switchDiffContext('commit', latestCommitSha)}
                      disabled={!latestCommitSha}>latest commit</button
                    >.</span
                  >
                </div>
              {/if}
            </div>

            {#if !readonly && isContextEditable && diffViewer.state.commitSha}
              <DiffCommitSessionLauncher
                {branchId}
                {projectId}
                commitSha={diffViewer.state.commitSha}
                scope={activeScope}
                reviewId={activeReviewId}
                visibleCommentCount={currentComments.length}
                onStarted={onClose}
              />
            {/if}
          </div>
        {/if}
      </div>
    </div>
  </div>
</div>

<!-- Delete comment confirmation -->
{#if commentToDelete}
  <ConfirmDialog
    title="Delete Comment"
    message="Are you sure you want to delete this comment?"
    confirmLabel="Delete"
    danger={true}
    onConfirm={confirmDeleteComment}
    onCancel={() => (commentToDelete = null)}
  />
{/if}

<!-- Delete all comments confirmation -->
{#if showDeleteAllConfirm}
  <ConfirmDialog
    title="Delete All Comments"
    message="Are you sure you want to delete all comments? This action cannot be undone."
    confirmLabel="Delete All"
    danger={true}
    onConfirm={confirmDeleteAllComments}
    onCancel={() => (showDeleteAllConfirm = false)}
  />
{/if}

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

  .icon-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 5px;
    background: transparent;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    -webkit-app-region: no-drag;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .icon-btn:hover:not(:disabled) {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .icon-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }

  /* ========================================================================
   * Context switcher
   * ====================================================================== */

  .context-switcher {
    position: relative;
    -webkit-app-region: no-drag;
  }

  .context-switcher-btn {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 4px 10px;
    background: none;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-weight: 500;
    font-family: inherit;
    cursor: pointer;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s;
  }

  .context-switcher-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-muted);
    background: var(--bg-hover);
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

  .context-info-box {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    margin: 16px 12px 8px;
    padding: 10px 12px;
    border-radius: 6px;
    background-color: var(--bg-hover);
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.4;
  }

  .context-link-btn {
    all: unset;
    color: var(--text-default);
    text-decoration: underline;
    cursor: pointer;
  }

  .context-link-btn:hover {
    color: var(--ui-primary);
  }

  .context-link-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .context-info-box :global(svg) {
    flex-shrink: 0;
    margin-top: 1px;
    color: var(--text-faint);
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
</style>
