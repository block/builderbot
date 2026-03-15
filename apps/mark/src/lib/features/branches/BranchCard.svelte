<!--
  BranchCard.svelte - Card display for a tracked branch

  Shows branch name, base branch, and a unified timeline of commits/notes/reviews.
  Footer has two buttons: "New note" and "New commit" for creating items.
  Opens a modal for prompt entry; draft text is preserved across open/close.

  Timeline items are clickable:
  - Commits open a limited diff view (no commenting / reference files)
  - Notes open a markdown viewer
  - Each item shows session + delete actions on hover
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { FileDiff, AlertCircle, Cloud, Trash2 } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { subscribeDragDrop } from './dragDrop';
  import type {
    Branch,
    BranchTimeline as BranchTimelineData,
    SessionStatusPayload,
    WorkspaceStatus,
  } from '../../types';
  import * as commands from '../../api/commands';
  import BranchTimeline from '../timeline/BranchTimeline.svelte';
  import ImageViewerModal from '../timeline/ImageViewerModal.svelte';
  import DiffModal from '../diff/DiffModal.svelte';
  import SessionModal from '../sessions/SessionModal.svelte';
  import NewSessionModal from '../sessions/NewSessionModal.svelte';
  import NoteModal from '../notes/NoteModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import { fileNameFromPath, formatBaseBranch, isTextFile, isImageFile } from './branchCardHelpers';
  import BranchCardHeaderInfo from './BranchCardHeaderInfo.svelte';
  import BranchCardActionsBar from './BranchCardActionsBar.svelte';
  import BranchCardPrButton from './BranchCardPrButton.svelte';
  import BranchCardSessionManager from './BranchCardSessionManager.svelte';
  import ReasonBanner from './ReasonBanner.svelte';
  import RemoteWorkspacePoller from './RemoteWorkspacePoller.svelte';
  import RemoteWorkspaceStatusBadge from './RemoteWorkspaceStatusBadge.svelte';
  import RemoteWorkspaceStatusView from './RemoteWorkspaceStatusView.svelte';
  import { alerts } from '../../shared/alerts.svelte';

  interface Props {
    branch: Branch;
    repoLabel?: { githubRepo: string; subpath: string | null; reason?: string | null } | null;
    projectName?: string;
    deleting?: boolean;
    worktreeError?: string;
    workspaceError?: string;
    onDelete?: () => void;
    onRename?: (branchName: string) => void;
    onRetryWorktree?: () => void;
    onWorkspaceStatusChange?: (status: WorkspaceStatus, workstationId?: number | null) => void;
  }

  let {
    branch,
    repoLabel = null,
    projectName,
    deleting = false,
    worktreeError,
    workspaceError,
    onDelete,
    onRename,
    onRetryWorktree,
    onWorkspaceStatusChange,
  }: Props = $props();

  // Determine if this is a local or remote branch
  const isLocal = $derived(branch.branchType === 'local');
  const isRemote = $derived(branch.branchType === 'remote');
  let remoteWorkspaceStatus = $state<WorkspaceStatus | null>(null);

  function notifyError(title: string, e: unknown): void {
    alerts.show({
      tone: 'error',
      title,
      message: e instanceof Error ? e.message : String(e),
      durationMs: 0,
    });
  }

  // =========================================================================
  // Timeline state
  // =========================================================================

  let timeline = $state<BranchTimelineData | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let showBranchDiff = $state(false);
  let loadedTimelineKey = $state<string | null>(null);
  type TimelineReviewDetails = {
    commitSha: string;
    scope: 'branch' | 'commit';
    comments: number;
    annotations: number;
    warnings: number;
  };
  let timelineReviewDetailsById = $state<Record<string, TimelineReviewDetails>>({});
  let reviewDetailsLoadVersion = 0;
  let reviewDiffTarget = $state<{
    commitSha: string;
    scope: 'branch' | 'commit';
    reviewId: string;
  } | null>(null);

  /** True when the branch has at least one finalized commit (code changes vs base). */
  let hasCodeChanges = $derived(timeline?.commits.some((c) => c.sha) ?? false);

  // Compute a suggested commit prompt from the latest visible timeline entry.
  // Only used when the user hasn't typed a draft yet.
  let commitPrefill = $derived.by(() => {
    if (!timeline) return '';

    // Find the latest completed item across commits, notes, and reviews
    type Candidate =
      | { kind: 'review'; commentCount: number; timestamp: number }
      | { kind: 'note'; title: string; timestamp: number };

    const candidates: Candidate[] = [];

    for (const review of timeline.reviews) {
      if (review.sessionStatus === 'running') continue;
      const ts = Math.floor(review.createdAt / 1000);
      candidates.push({ kind: 'review', commentCount: review.commentCount, timestamp: ts });
    }

    for (const note of timeline.notes) {
      if (note.sessionStatus === 'running') continue;
      const ts = Math.floor(note.createdAt / 1000);
      candidates.push({ kind: 'note', title: note.title, timestamp: ts });
    }

    // We also need commits so we can tell if the latest item overall is a commit
    // (in which case we return blank).
    type AnyCandidate = Candidate | { kind: 'commit'; timestamp: number };
    const all: AnyCandidate[] = [...candidates];
    for (const commit of timeline.commits) {
      if (!commit.sha) continue; // skip pending
      all.push({ kind: 'commit', timestamp: commit.timestamp });
    }

    if (all.length === 0) return '';

    all.sort((a, b) => b.timestamp - a.timestamp);
    const latest = all[0];

    if (latest.kind === 'review' && latest.commentCount > 0) {
      return 'Resolve code review comments';
    }
    if (latest.kind === 'note' && latest.title.toLowerCase().includes('plan')) {
      return 'Implement plan';
    }

    return '';
  });

  // Commit diff modal (opened by clicking a commit in the timeline)
  let commitDiffSha = $state<string | null>(null);

  // Note modal (opened by clicking a note in the timeline)
  let openNote = $state<{ title: string; content: string } | null>(null);

  // Image viewer modal (opened by clicking an image in the timeline)
  let viewImageId = $state<string | null>(null);
  let viewImageFilename = $state<string>('');
  let deletingImageIds = $state<Set<string>>(new Set());
  let timelineDeletingItems = $derived(
    [...deletingImageIds].map((id) => ({ type: 'image' as const, id }))
  );

  // Confirm delete dialog
  let confirmDelete = $state<{
    title: string;
    message: string;
    onConfirm: () => void;
  } | null>(null);

  // =========================================================================
  // Session manager (reactive .svelte.ts module)
  // =========================================================================

  const sessionMgr = new BranchCardSessionManager({
    getBranch: () => branch,
    getIsRemote: () => isRemote,
    loadTimeline: () => loadTimeline(),
    getTimeline: () => timeline,
  });

  /** Number of finalized commits on this branch. */
  let commitCount = $derived(timeline?.commits.filter((c) => c.sha).length ?? 0);

  // =========================================================================
  // PR button ref
  // =========================================================================
  let prButton = $state<ReturnType<typeof BranchCardPrButton> | undefined>();

  // =========================================================================
  // Actions bar ref
  // =========================================================================
  let actionsBar = $state<ReturnType<typeof BranchCardActionsBar> | undefined>();

  // =========================================================================
  // Event listeners
  // =========================================================================

  let unlistenStatus: UnlistenFn | null = null;

  $effect(() => {
    const branchId = branch.id;

    listen<SessionStatusPayload>('session-status-changed', (event) => {
      const {
        sessionId: eventSessionId,
        status,
        branchId: eventBranchId,
        isAutoReview,
      } = event.payload;
      if (status === 'completed' || status === 'error' || status === 'cancelled') {
        // If this is the auto review session completing, just clear tracking
        if (eventSessionId === sessionMgr.autoReviewSessionId) {
          sessionMgr.autoReviewSessionId = null;
          return;
        }

        // Skip normal completion handling for any auto review session
        if (isAutoReview) {
          return;
        }

        sessionMgr.pendingSessionItems = sessionMgr.pendingSessionItems.filter(
          (item) => item.sessionId !== eventSessionId
        );
        loadTimeline();
        // Handle PR session completion
        if (prButton && eventSessionId === prButton.getPrSessionId()) {
          prButton.handlePrSessionComplete(status);
        }
        // Handle push session completion
        if (prButton && eventSessionId === prButton.getPushSessionId()) {
          prButton.handlePushSessionComplete(status);
        }
      } else if (status === 'running' && eventBranchId === branchId) {
        // Track auto review sessions started by the backend
        if (isAutoReview) {
          sessionMgr.autoReviewSessionId = eventSessionId;
          commands.findFreshAutoReview(branchId).then((review) => {
            if (review) {
              sessionMgr.autoReviewId = review.id;
            }
          });
        }
        // Refresh the timeline so the pending note/commit stub appears immediately
        if (!isAutoReview) {
          loadTimeline();
        }
      }
    }).then((unlisten) => {
      unlistenStatus = unlisten;
    });

    return () => {
      unlistenStatus?.();
    };
  });

  // Load timeline when a branch becomes timeline-ready
  $effect(() => {
    if (isLocal && !branch.worktreePath) return;
    if (isRemote && remoteWorkspaceStatus !== 'running') return;

    const timelineKey = isRemote ? `${branch.id}:<remote>` : `${branch.id}:${branch.worktreePath}`;
    if (timelineKey === loadedTimelineKey) return;

    loadedTimelineKey = timelineKey;
    void loadTimeline();
  });

  async function loadTimeline() {
    const isInitialLoad = !timeline;
    if (isInitialLoad) {
      loading = true;
    }
    error = null;
    try {
      const nextTimeline = await commands.getBranchTimeline(branch.id, { force: !isInitialLoad });
      timeline = nextTimeline;
      sessionMgr.prunePendingSessionItems(nextTimeline);
      void loadTimelineReviewDetails(nextTimeline.reviews);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function loadTimelineReviewDetails(reviews: BranchTimelineData['reviews']) {
    const loadVersion = ++reviewDetailsLoadVersion;
    if (reviews.length === 0) {
      timelineReviewDetailsById = {};
      return;
    }

    const reviewDetails = await Promise.all(
      reviews.map(async (review) => {
        try {
          const fullReview = await commands.getReview(review.id);
          if (!fullReview) return null;

          let comments = 0;
          let annotations = 0;
          let warnings = 0;
          for (const comment of fullReview.comments) {
            if (comment.commentType === 'information') {
              annotations += 1;
            } else {
              comments += 1;
              if (comment.commentType === 'warning') {
                warnings += 1;
              }
            }
          }

          const details: TimelineReviewDetails = {
            commitSha: fullReview.commitSha,
            scope: fullReview.scope,
            comments,
            annotations,
            warnings,
          };
          return { id: review.id, details };
        } catch (e) {
          console.error(`Failed to load review details for ${review.id}:`, e);
          return null;
        }
      })
    );

    if (loadVersion !== reviewDetailsLoadVersion) return;

    const nextDetails: Record<string, TimelineReviewDetails> = {};
    for (const item of reviewDetails) {
      if (!item) continue;
      nextDetails[item.id] = item.details;
    }
    timelineReviewDetailsById = nextDetails;
  }

  // =========================================================================
  // Timeline item interactions
  // =========================================================================

  function handleCommitClick(sha: string) {
    commitDiffSha = sha;
  }

  function handleNoteClick(_noteId: string, title: string, content: string) {
    openNote = { title, content };
  }

  async function handleReviewClick(reviewId: string) {
    const cached = timelineReviewDetailsById[reviewId];
    if (cached) {
      reviewDiffTarget = { commitSha: cached.commitSha, scope: cached.scope, reviewId };
      showBranchDiff = true;
      return;
    }

    try {
      const review = await commands.getReview(reviewId);
      if (!review) {
        notifyError('Review not found', 'This review no longer exists.');
        await loadTimeline();
        return;
      }
      reviewDiffTarget = { commitSha: review.commitSha, scope: review.scope, reviewId };
      showBranchDiff = true;
    } catch (e) {
      console.error('Failed to open review:', e);
      notifyError('Failed to open review', e);
    }
  }

  function handleDeleteCommit(sha: string, sessionId?: string) {
    confirmDelete = {
      title: 'Delete Commit',
      message:
        'This will reset the branch to the parent commit, removing this commit and its changes.' +
        (sessionId ? ' The linked session will also be deleted.' : ''),
      onConfirm: async () => {
        confirmDelete = null;
        try {
          await commands.deleteCommit(branch.id, sha, !!sessionId);
          loadTimeline();
        } catch (e) {
          console.error('Failed to delete commit:', e);
          notifyError('Failed to delete commit', e);
        }
      },
    };
  }

  function handleDeleteNote(noteId: string, sessionId?: string) {
    confirmDelete = {
      title: 'Delete Note',
      message:
        'Are you sure you want to delete this note?' +
        (sessionId ? ' The linked session will also be deleted.' : ''),
      onConfirm: async () => {
        confirmDelete = null;
        try {
          if (sessionId) {
            try {
              await commands.cancelSession(sessionId);
            } catch {
              // Session may already be finished
            }
          }
          await commands.deleteNote(noteId, !!sessionId);
          loadTimeline();
        } catch (e) {
          console.error('Failed to delete note:', e);
          notifyError('Failed to delete note', e);
        }
      },
    };
  }

  function handleDeleteReview(reviewId: string, sessionId?: string) {
    confirmDelete = {
      title: 'Delete Review',
      message:
        'Are you sure you want to delete this review and all its comments?' +
        (sessionId ? ' The linked session will also be deleted.' : ''),
      onConfirm: async () => {
        confirmDelete = null;
        try {
          if (sessionId) {
            try {
              await commands.cancelSession(sessionId);
            } catch {
              // Session may already be finished
            }
          }
          await commands.deleteReview(reviewId, !!sessionId);
          loadTimeline();
        } catch (e) {
          console.error('Failed to delete review:', e);
          notifyError('Failed to delete review', e);
        }
      },
    };
  }

  async function handleDeletePendingCommit(commitId: string, sessionId?: string) {
    try {
      if (sessionId) {
        try {
          await commands.cancelSession(sessionId);
        } catch {
          // Session may already be finished, that's fine
        }
      }
      await commands.deletePendingCommit(commitId, !!sessionId);
      loadTimeline();
    } catch (e) {
      console.error('Failed to delete pending commit:', e);
      notifyError('Failed to delete pending commit', e);
    }
  }

  function handleImageClick(imageId: string) {
    const image = timeline?.images.find((img) => img.id === imageId);
    if (image) {
      viewImageId = imageId;
      viewImageFilename = image.filename;
    }
  }

  function handleDeleteImage(imageId: string) {
    confirmDelete = {
      title: 'Delete Image',
      message: 'Are you sure you want to delete this image?',
      onConfirm: async () => {
        confirmDelete = null;
        deletingImageIds = new Set([...deletingImageIds, imageId]);
        if (viewImageId === imageId) {
          viewImageId = null;
        }
        try {
          await commands.deleteImage(imageId);
          loadTimeline();
        } catch (e) {
          console.error('Failed to delete image:', e);
          notifyError('Failed to delete image', e);
        } finally {
          deletingImageIds = new Set([...deletingImageIds].filter((id) => id !== imageId));
        }
      },
    };
  }

  // =========================================================================
  // Repo reason banner
  // =========================================================================

  async function handleDismissReason() {
    if (branch.projectRepoId) {
      try {
        await commands.clearProjectRepoReason(branch.projectRepoId);
      } catch (e) {
        console.error('Failed to clear repo reason:', e);
      }
    }
  }

  // =========================================================================
  // Drag-and-drop text files → notes (via Tauri native drag-drop events)
  // =========================================================================

  let dragOver = $state(false);
  let cardElement: HTMLDivElement | undefined = $state();

  let pendingDropNotes = $state<{ key: string; title: string }[]>([]);

  function handleFileDrop(paths: string[]) {
    const textPaths = paths.filter(isTextFile);
    const imagePaths = paths.filter(isImageFile);

    if (textPaths.length > 0) {
      const placeholders = textPaths.map((filePath) => ({
        key: `drop-${Date.now()}-${filePath}`,
        title: fileNameFromPath(filePath),
      }));
      pendingDropNotes = [...pendingDropNotes, ...placeholders];

      Promise.all(
        textPaths.map(async (filePath, i) => {
          try {
            const content = await commands.readTextFile(filePath);
            const title = fileNameFromPath(filePath);
            await commands.createNote(branch.id, title, content);
          } catch (e) {
            console.error('Failed to create note from dropped file:', e);
          } finally {
            pendingDropNotes = pendingDropNotes.filter((p) => p.key !== placeholders[i].key);
          }
        })
      ).then(() => {
        loadTimeline();
      });
    }

    if (imagePaths.length > 0) {
      Promise.all(
        imagePaths.map(async (filePath) => {
          try {
            await commands.createImage(branch.id, branch.projectId, filePath);
          } catch (e) {
            console.error('Failed to create image from drop:', e);
          }
        })
      ).then(() => {
        loadTimeline();
      });
    }
  }

  $effect(() => {
    if (!isLocal) return;

    const el = cardElement;
    if (!el) return;

    const unsub = untrack(() =>
      subscribeDragDrop({
        element: el,
        onDragOver: (over) => {
          dragOver = over;
        },
        onDrop: (paths) => {
          handleFileDrop(paths);
        },
      })
    );

    return unsub;
  });
</script>

<svelte:window onclick={(e) => actionsBar?.handleClickOutside(e)} />
{#if isRemote}
  <RemoteWorkspacePoller
    branchId={branch.id}
    incomingStatus={branch.workspaceStatus}
    bind:status={remoteWorkspaceStatus}
    onStatusChange={(status, workstationId) => onWorkspaceStatusChange?.(status, workstationId)}
  />
{/if}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  bind:this={cardElement}
  class="branch-card"
  class:deleting
  class:creating-worktree={isLocal && !branch.worktreePath && !worktreeError && !deleting}
  data-branch-id={branch.id}
  class:drag-over={dragOver}
>
  {#if deleting}
    <div class="deleting-overlay">
      <Spinner size={16} />
      <span>Deleting…</span>
    </div>
  {:else if isLocal && !branch.worktreePath}
    <div class="card-header">
      <BranchCardHeaderInfo
        branchName={branch.branchName}
        {repoLabel}
        secondaryLabel={formatBaseBranch(branch.baseBranch)}
      />
      {#if worktreeError}
        <div class="header-actions">
          <button class="more-button" onclick={() => onDelete?.()} title="Delete branch">
            <Trash2 size={16} />
          </button>
        </div>
      {/if}
    </div>
    <div class="card-content">
      {#if worktreeError}
        <div class="worktree-error">
          <div class="worktree-error-message">
            <AlertCircle size={14} />
            <span>Failed to create worktree: {worktreeError}</span>
          </div>
          <button class="worktree-retry-btn" onclick={() => onRetryWorktree?.()}> Retry </button>
        </div>
      {:else}
        <div class="loading">
          <Spinner size={14} />
          <span>Creating worktree…</span>
        </div>
      {/if}
    </div>
  {:else}
    <div class="card-header">
      {#if isRemote}
        <Cloud size={14} class="header-icon cloud-icon" />
      {/if}
      <BranchCardHeaderInfo
        branchName={branch.branchName}
        {repoLabel}
        secondaryLabel={isRemote
          ? (branch.workspaceName ?? formatBaseBranch(branch.baseBranch))
          : formatBaseBranch(branch.baseBranch)}
      />
      <div class="header-actions">
        {#if isRemote && remoteWorkspaceStatus !== 'running'}
          <RemoteWorkspaceStatusBadge status={remoteWorkspaceStatus} />
        {/if}
        <BranchCardActionsBar
          bind:this={actionsBar}
          {branch}
          {repoLabel}
          {isLocal}
          {isRemote}
          {remoteWorkspaceStatus}
          {onDelete}
          {onRename}
          onNoteCreated={() => loadTimeline()}
          onRebaseBranch={() =>
            sessionMgr.startBranchSessionWithPendingItem('commit', 'Rebase this branch')}
          onCollapseCommits={() =>
            sessionMgr.startBranchSessionWithPendingItem(
              'commit',
              "Collapse this branch's commits"
            )}
          newCommitDisabled={sessionMgr.isNewSessionDisabled}
          {commitCount}
        />
      </div>
    </div>

    <div class="card-content">
      <ReasonBanner reason={repoLabel?.reason} onDismiss={handleDismissReason} />
      {#if isRemote && (remoteWorkspaceStatus === 'starting' || remoteWorkspaceStatus === 'stopped' || remoteWorkspaceStatus === 'error')}
        <RemoteWorkspaceStatusView
          status={remoteWorkspaceStatus}
          {workspaceError}
          fallbackError={error}
        />
      {:else if loading}
        <div class="loading">
          <Spinner size={14} />
          <span>Loading...</span>
        </div>
      {:else if error}
        <div class="error">
          <span>{error}</span>
        </div>
      {:else if timeline}
        <BranchTimeline
          {timeline}
          repoDir={branch.worktreePath}
          pendingDropNotes={isLocal ? pendingDropNotes : undefined}
          pendingItems={sessionMgr.pendingSessionItems}
          deletingItems={timelineDeletingItems}
          reviewCommentBreakdown={timelineReviewDetailsById}
          onSessionClick={(sid) => sessionMgr.handleTimelineSessionClick(sid)}
          onCommitClick={handleCommitClick}
          onNoteClick={handleNoteClick}
          onReviewClick={handleReviewClick}
          onDeleteCommit={handleDeleteCommit}
          onDeletePendingCommit={handleDeletePendingCommit}
          onDeleteNote={handleDeleteNote}
          onDeleteReview={handleDeleteReview}
          onImageClick={handleImageClick}
          onDeleteImage={handleDeleteImage}
          onNewNote={() => sessionMgr.openNewSession('note')}
          onNewCommit={() => sessionMgr.openNewSession('commit')}
          onNewReview={hasCodeChanges ? (e) => sessionMgr.openNewSession('review', e) : undefined}
          newSessionDisabled={sessionMgr.isNewSessionDisabled}
        >
          {#snippet footerActions()}
            {#if hasCodeChanges}
              <div class="footer-right-actions">
                <BranchCardPrButton
                  bind:this={prButton}
                  {branch}
                  {isLocal}
                  {isRemote}
                  {hasCodeChanges}
                  {timeline}
                  onOpenSession={(sid) => {
                    sessionMgr.openSessionId = sid;
                  }}
                />
                <button
                  class="pr-btn diff-btn"
                  onclick={() => {
                    reviewDiffTarget = null;
                    showBranchDiff = true;
                  }}
                  title="View diff"
                >
                  <FileDiff size={13} />
                  <span>Diff</span>
                </button>
              </div>
            {/if}
          {/snippet}
        </BranchTimeline>
      {/if}
    </div>
  {/if}
</div>

{#if showBranchDiff}
  <DiffModal
    branchId={branch.id}
    commitSha={reviewDiffTarget?.commitSha}
    scope={reviewDiffTarget?.scope ?? 'branch'}
    reviewId={reviewDiffTarget?.reviewId}
    beforeLabel={reviewDiffTarget?.scope === 'commit'
      ? 'parent'
      : formatBaseBranch(branch.baseBranch)}
    afterLabel={reviewDiffTarget?.commitSha
      ? reviewDiffTarget.commitSha.slice(0, 7)
      : branch.branchName}
    {projectName}
    githubRepo={repoLabel?.githubRepo}
    subpath={repoLabel?.subpath}
    onClose={() => {
      showBranchDiff = false;
      reviewDiffTarget = null;
      loadTimeline();
    }}
  />
{/if}

{#if commitDiffSha}
  <DiffModal
    branchId={branch.id}
    commitSha={commitDiffSha}
    scope="commit"
    beforeLabel="parent"
    afterLabel={commitDiffSha.slice(0, 7)}
    {projectName}
    githubRepo={repoLabel?.githubRepo}
    subpath={repoLabel?.subpath}
    readonly
    onClose={() => {
      commitDiffSha = null;
      loadTimeline();
    }}
  />
{/if}

{#if openNote}
  <NoteModal title={openNote.title} content={openNote.content} onClose={() => (openNote = null)} />
{/if}

{#if viewImageId}
  <ImageViewerModal
    imageId={viewImageId}
    filename={viewImageFilename}
    onClose={() => {
      viewImageId = null;
    }}
    onDelete={() => {
      if (viewImageId) {
        handleDeleteImage(viewImageId);
      }
    }}
  />
{/if}

{#if sessionMgr.showNewSession}
  {@const usePrefill =
    sessionMgr.newSessionMode === 'commit' && !sessionMgr.draftPrompt && !!commitPrefill}
  <NewSessionModal
    {branch}
    mode={sessionMgr.newSessionMode}
    initialPrompt={usePrefill ? commitPrefill : sessionMgr.draftPrompt}
    initialImageIds={sessionMgr.draftImageIds}
    prefilled={usePrefill}
    remote={isRemote}
    onClose={(draft) => {
      // Don't persist prefilled text as a draft — it should be re-evaluated
      // each time the dialog opens based on the current timeline state.
      const prompt = draft.prompt === commitPrefill ? '' : draft.prompt;
      sessionMgr.handleNewSessionClose({ ...draft, prompt });
    }}
    onSubmit={(data) => sessionMgr.handleNewSessionSubmit(data)}
  />
{/if}

{#if sessionMgr.openSessionId}
  <SessionModal
    sessionId={sessionMgr.openSessionId}
    repoDir={branch.worktreePath}
    branchId={branch.id}
    projectId={branch.projectId}
    onClose={async () => {
      const closedSessionId = sessionMgr.openSessionId;
      sessionMgr.openSessionId = null;
      loadTimeline();
      // If the closed modal was the PR session, check if it finished while open
      if (
        prButton &&
        closedSessionId &&
        closedSessionId === prButton.getPrSessionId() &&
        prButton.getPrCreatingState() === 'creating'
      ) {
        try {
          const session = await commands.getSession(closedSessionId);
          if (session && session.status !== 'running') {
            prButton.handlePrSessionComplete(session.status);
          }
        } catch {
          // Ignore — the polling fallback will catch it
        }
      }
      // If the closed modal was the push session, check if it finished while open
      if (
        prButton &&
        closedSessionId &&
        closedSessionId === prButton.getPushSessionId() &&
        prButton.getPushingState() === 'pushing'
      ) {
        try {
          const session = await commands.getSession(closedSessionId);
          if (session && session.status !== 'running') {
            prButton.handlePushSessionComplete(session.status);
          }
        } catch {
          // Ignore — the polling fallback will catch it
        }
      }
    }}
  />
{/if}

{#if confirmDelete}
  <ConfirmDialog
    title={confirmDelete.title}
    message={confirmDelete.message}
    confirmLabel="Delete"
    danger
    onConfirm={confirmDelete.onConfirm}
    onCancel={() => (confirmDelete = null)}
  />
{/if}

<style>
  .branch-card {
    display: flex;
    flex-direction: column;
    background-color: var(--bg-primary);
    border-radius: 8px;
    border: 1px solid var(--border-subtle);
  }

  .branch-card.deleting {
    opacity: 0.6;
  }

  /* Drag-and-drop highlight */
  .branch-card.drag-over {
    border-color: var(--note-color, var(--ui-accent));
    background-color: color-mix(in srgb, var(--note-color, var(--ui-accent)) 5%, var(--bg-primary));
  }

  /* Deleting overlay */
  .deleting-overlay {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 20px 16px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  /* Header */
  .card-header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    min-width: 0;
    border-bottom: 1px solid var(--border-subtle);
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .card-header :global(svg.header-icon) {
    flex-shrink: 0;
    stroke: var(--text-faint);
  }

  .card-header :global(svg.cloud-icon) {
    stroke: var(--ui-accent);
  }

  .more-button {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 6px;
    background: transparent;
    border: none;
    border-radius: 6px;
    color: var(--text-faint);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .more-button:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  :global(.branch-icon) {
    color: var(--branch-color);
    flex-shrink: 0;
  }

  /* Content */
  .card-content {
    padding: 16px;
    min-height: 80px;
  }

  .loading {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .error {
    color: var(--ui-danger);
    font-size: var(--size-sm);
  }

  /* Worktree error state */
  .worktree-error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .worktree-error-message {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    color: var(--ui-danger);
    font-size: var(--size-sm);
    line-height: 1.4;
    min-width: 0;
  }

  .worktree-error-message :global(svg) {
    flex-shrink: 0;
    margin-top: 1px;
  }

  .worktree-retry-btn {
    flex-shrink: 0;
    padding: 5px 14px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .worktree-retry-btn:hover {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
    background-color: var(--bg-hover);
  }

  /* Footer right actions (PR and diff buttons) */
  .footer-right-actions {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  /* Diff button (reuses pr-btn style) */
  .pr-btn {
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
    cursor: pointer;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s;
    white-space: nowrap;
  }

  .pr-btn:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--border-muted);
    background: var(--bg-hover);
  }

  .pr-btn :global(svg) {
    flex-shrink: 0;
  }
</style>
