<!--
  RemoteBranchCard.svelte - Card display for a remote Blox workspace branch

  Shows branch name, workspace status badge, agent type, and — when the
  workspace is running — a full timeline + session UI matching BranchCard.

  Lifecycle:
  - Starting: shows spinner, polls every 3s until Running
  - Running: shows timeline, New button (commit/note via blox acp sessions)
  - Stopped: shows restart hint
  - Error: shows error state
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import {
    Cloud,
    Trash2,
    AlertCircle,
    CircleCheck,
    CirclePause,
    Copy,
    Pencil,
    FileDiff,
  } from 'lucide-svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import Spinner from '../../shared/Spinner.svelte';
  import type {
    Branch,
    BranchTimeline as BranchTimelineData,
    BranchSessionType,
    WorkspaceInfo,
    WorkspaceStatus,
  } from '../../types';
  import * as commands from '../../api/commands';
  import BranchTimeline from '../timeline/BranchTimeline.svelte';
  import DropdownMenu, { type MenuItem } from '../../shared/DropdownMenu.svelte';
  import DiffModal from '../diff/DiffModal.svelte';
  import SessionModal from '../sessions/SessionModal.svelte';
  import NewSessionModal from '../sessions/NewSessionModal.svelte';
  import NoteModal from '../notes/NoteModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import BranchCardHeaderInfo from './BranchCardHeaderInfo.svelte';
  import ReasonBanner from './ReasonBanner.svelte';
  import { formatBaseBranch } from './branchCardHelpers';
  import { alerts } from '../../shared/alerts.svelte';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import { sessionRegistry } from '../../stores/sessionRegistry.svelte';

  interface Props {
    branch: Branch;
    repoLabel?: { githubRepo: string; subpath: string | null; reason?: string | null } | null;
    projectName?: string;
    deleting?: boolean;
    workspaceError?: string;
    onDelete?: () => void;
    onRename?: (branchName: string) => void;
    onWorkspaceStatusChange?: (status: WorkspaceStatus) => void;
  }

  let {
    branch,
    repoLabel = null,
    projectName,
    deleting = false,
    workspaceError = undefined,
    onDelete,
    onRename,
    onWorkspaceStatusChange,
  }: Props = $props();

  function notifyError(title: string, e: unknown): void {
    alerts.show({
      tone: 'error',
      title,
      message: e instanceof Error ? e.message : String(e),
      durationMs: 0,
    });
  }

  // Reactive workspace status (updated by polling)
  let polledStatus = $state<WorkspaceStatus | null>(null);
  let status = $derived<WorkspaceStatus | null>(polledStatus ?? branch.workspaceStatus);
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let pollStartedAt: number | null = null;
  let longProvisioning = $state(false);
  let pollInFlight = false;
  let workspaceUrl = $state<string | null>(null);
  let workspaceInfoRequestId = 0;
  const LONG_PROVISIONING_MS = 5 * 60 * 1000; // 5 minutes

  // Error state
  let error = $state<string | null>(null);

  $effect(() => {
    if (workspaceError && workspaceError !== error) {
      error = workspaceError;
    }
  });

  // Timeline state
  let timeline = $state<BranchTimelineData | null>(null);
  let timelineLoading = $state(true);
  let timelineError = $state<string | null>(null);
  let pendingTimelineItems = $state<
    {
      key: string;
      type: 'pending-commit' | 'generating-note' | 'generating-review';
      title: string;
      secondaryMeta: string;
      sessionId?: string;
    }[]
  >([]);
  let deletingTimelineItems = $state<{ type: 'commit' | 'note' | 'review'; id: string }[]>([]);

  // New session modal state
  let showNewSession = $state(false);
  let newSessionMode = $state<BranchSessionType>('commit');
  let draftPrompt = $state('');

  // Session modal (opened from timeline or after starting a session)
  let openSessionId = $state<string | null>(null);

  // Note modal
  let openNote = $state<{ title: string; content: string } | null>(null);

  // Branch diff modal
  let showBranchDiff = $state(false);
  let commitDiffSha = $state<string | null>(null);
  type TimelineReviewDetails = {
    commitSha: string;
    scope: 'branch' | 'commit';
    comments: number;
    annotations: number;
  };
  let timelineReviewDetailsById = $state<Record<string, TimelineReviewDetails>>({});
  let reviewDetailsLoadVersion = 0;
  let reviewDiffTarget = $state<{ commitSha: string; scope: 'branch' | 'commit' } | null>(null);

  // Confirm delete dialog
  let confirmDelete = $state<{
    title: string;
    message: string;
    onConfirm: () => void;
  } | null>(null);

  /** True when the branch has at least one finalized commit (code changes vs base). */
  let hasCodeChanges = $derived(timeline?.commits.some((c) => !!c.sha) ?? false);

  // Listen for session completion to refresh timeline
  let unlistenStatus: UnlistenFn | null = null;

  const menuItems: MenuItem[] = $derived([
    ...(branch.workspaceName
      ? [
          {
            label: 'Copy Workspace Name',
            icon: Copy,
            action: () => copyText(branch.workspaceName!),
          },
        ]
      : []),
    {
      label: 'Rename Branch',
      icon: Pencil,
      action: () => {
        const next = window.prompt('Rename branch', branch.branchName);
        if (!next) return;
        const trimmed = next.trim();
        if (!trimmed || trimmed === branch.branchName) return;
        onRename?.(trimmed);
      },
    },
    {
      label: 'Delete Repo',
      icon: Trash2,
      danger: true,
      action: () => {
        onDelete?.();
      },
    },
  ]);

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // clipboard API may fail
    }
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
  // Status polling
  // =========================================================================

  onMount(() => {
    if (status === 'starting') {
      startPolling();
    }
    if (status === 'running') {
      loadTimeline();
      loadWorkspaceUrl();
    }
    listenForStatusChanges();
  });

  onDestroy(() => {
    stopPolling();
    unlistenStatus?.();
  });

  function startPolling() {
    stopPolling();
    pollStartedAt = Date.now();
    longProvisioning = false;
    pollTimer = setInterval(async () => {
      if (pollInFlight) {
        return;
      }

      if (pollStartedAt && Date.now() - pollStartedAt > LONG_PROVISIONING_MS) {
        longProvisioning = true;
      }

      pollInFlight = true;
      try {
        const newStatus = (await commands.pollWorkspaceStatus(branch.id)) as WorkspaceStatus;
        polledStatus = newStatus;
        onWorkspaceStatusChange?.(newStatus);
        if (newStatus === 'running') {
          error = null;
          longProvisioning = false;
          stopPolling();
          loadTimeline();
          loadWorkspaceUrl();
        } else if (newStatus !== 'starting') {
          longProvisioning = false;
          stopPolling();
          workspaceUrl = null;
        }
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);

        // Auth errors are definitive — stop polling and show an actionable message.
        if (isAuthError(msg)) {
          console.error('Blox authentication error:', msg);
          polledStatus = 'error';
          onWorkspaceStatusChange?.('error');
          error = msg;
          stopPolling();
          return;
        }

        // During initial creation, `blox ws start` may still be running
        // when the first poll fires and `blox ws info` can report
        // "workspace not found". Keep polling for those expected transients.
        if (status === 'starting' && isTransientStartupPollError(msg)) {
          console.debug('Poll failed while starting (workspace may not exist yet), retrying…', e);
        } else {
          console.error('Failed to poll workspace status:', e);
          polledStatus = 'error';
          onWorkspaceStatusChange?.('error');
          error = msg;
          stopPolling();
        }
      } finally {
        pollInFlight = false;
      }
    }, 3000);
  }

  function isAuthError(msg: string): boolean {
    const lower = msg.toLowerCase();
    return (
      lower.includes('not authenticated') ||
      lower.includes('not logged in') ||
      lower.includes('sq login')
    );
  }

  function isTransientStartupPollError(msg: string): boolean {
    const lower = msg.toLowerCase();
    return (
      lower.includes('not found') ||
      lower.includes('does not exist') ||
      lower.includes('no such') ||
      lower.includes('starting') ||
      lower.includes('provisioning')
    );
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
    pollInFlight = false;
  }

  let retrying = $state(false);

  async function retryWorkspace() {
    retrying = true;
    error = null;
    polledStatus = 'starting';
    longProvisioning = false;
    workspaceUrl = null;

    try {
      await commands.startWorkspace(branch.id);
      // If startWorkspace succeeds (or returns Ok), start polling
      startPolling();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      polledStatus = 'error';
      onWorkspaceStatusChange?.('error');
      error = msg;
    } finally {
      retrying = false;
    }
  }

  // =========================================================================
  // Timeline
  // =========================================================================

  async function listenForStatusChanges() {
    unlistenStatus = await listen<{
      sessionId: string;
      status: string;
    }>('session-status-changed', (event) => {
      const { sessionId: eventSessionId, status } = event.payload;
      if (status === 'completed' || status === 'error' || status === 'cancelled') {
        pendingTimelineItems = pendingTimelineItems.filter(
          (item) => item.sessionId !== eventSessionId
        );
        loadTimeline();
      }
    });
  }

  async function loadTimeline() {
    // Match BranchCard behavior: only block with loading/error when we have
    // no timeline yet. Background refreshes keep existing items visible.
    const isInitialLoad = !timeline;
    if (isInitialLoad) {
      timelineLoading = true;
      timelineError = null;
    }
    try {
      const nextTimeline = await commands.getBranchTimeline(branch.id, { force: !isInitialLoad });
      timeline = nextTimeline;
      void loadTimelineReviewDetails(nextTimeline.reviews);
      timelineError = null;
      // Drop optimistic placeholders once their real timeline entries exist.
      const seenSessionIds = new Set<string>();
      for (const commit of timeline.commits) {
        if (commit.sessionId) seenSessionIds.add(commit.sessionId);
      }
      for (const note of timeline.notes) {
        if (note.sessionId) seenSessionIds.add(note.sessionId);
      }
      for (const review of timeline.reviews) {
        if (review.sessionId) seenSessionIds.add(review.sessionId);
      }
      pendingTimelineItems = pendingTimelineItems.filter(
        (item) => !(item.sessionId && seenSessionIds.has(item.sessionId))
      );
    } catch (e) {
      if (isInitialLoad) {
        timelineError = e instanceof Error ? e.message : String(e);
      } else {
        console.error('Failed to refresh timeline:', e);
      }
    } finally {
      timelineLoading = false;
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
          for (const comment of fullReview.comments) {
            if (comment.commentType === 'information') {
              annotations += 1;
            } else {
              comments += 1;
            }
          }

          const details: TimelineReviewDetails = {
            commitSha: fullReview.commitSha,
            scope: fullReview.scope,
            comments,
            annotations,
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
  // New session modal
  // =========================================================================

  function openNewSession(mode: BranchSessionType) {
    newSessionMode = mode;
    showNewSession = true;
  }

  function handleNewSessionClose(draft: { prompt: string; mode: BranchSessionType }) {
    draftPrompt = draft.prompt;
    newSessionMode = draft.mode;
    showNewSession = false;
  }

  function handleNewSessionStarted(result: { sessionId: string; artifactId: string }) {
    if (!result?.sessionId) {
      console.error('Failed to start session: missing sessionId in result', result);
      notifyError('Session Error', 'Failed to start session: no session ID returned');
      return;
    }
    // Register session in the unified registry with the actual session type so global
    // completion handling can clear running/unread indicators for remote projects.
    sessionRegistry.register(result.sessionId, branch.projectId, newSessionMode, branch.id);
    // Track the running session in the project state store
    projectStateStore.addRunningSession(branch.projectId, result.sessionId);
    const pendingType =
      newSessionMode === 'commit'
        ? 'pending-commit'
        : newSessionMode === 'review'
          ? 'generating-review'
          : 'generating-note';
    const pendingTitle =
      newSessionMode === 'commit'
        ? 'Preparing commit...'
        : newSessionMode === 'review'
          ? 'Preparing code review...'
          : 'Preparing note...';
    pendingTimelineItems = [
      ...pendingTimelineItems,
      {
        key: `session-${result.sessionId}`,
        type: pendingType,
        title: pendingTitle,
        secondaryMeta: 'starting...',
        sessionId: result.sessionId,
      },
    ];
    showNewSession = false;
    draftPrompt = '';
    loadTimeline();
  }

  // =========================================================================
  // Timeline item interactions
  // =========================================================================

  function handleTimelineSessionClick(sessionId: string) {
    openSessionId = sessionId;
  }

  function handleNoteClick(_noteId: string, title: string, content: string) {
    openNote = { title, content };
  }

  async function handleReviewClick(reviewId: string) {
    const cached = timelineReviewDetailsById[reviewId];
    if (cached) {
      reviewDiffTarget = { commitSha: cached.commitSha, scope: cached.scope };
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
      reviewDiffTarget = { commitSha: review.commitSha, scope: review.scope };
      showBranchDiff = true;
    } catch (e) {
      console.error('Failed to open review:', e);
      notifyError('Failed to open review', e);
    }
  }

  function handleCommitClick(sha: string) {
    commitDiffSha = sha;
  }

  function markItemDeleting(type: 'commit' | 'note' | 'review', id: string) {
    if (!deletingTimelineItems.some((item) => item.type === type && item.id === id)) {
      deletingTimelineItems = [...deletingTimelineItems, { type, id }];
    }
  }

  function clearItemDeleting(type: 'commit' | 'note' | 'review', id: string) {
    deletingTimelineItems = deletingTimelineItems.filter(
      (item) => item.type !== type || item.id !== id
    );
  }

  function handleDeleteNote(noteId: string, sessionId?: string) {
    confirmDelete = {
      title: 'Delete Note',
      message:
        'Are you sure you want to delete this note?' +
        (sessionId ? ' The linked session will also be deleted.' : ''),
      onConfirm: async () => {
        confirmDelete = null;
        markItemDeleting('note', noteId);
        try {
          if (sessionId) {
            try {
              await commands.cancelSession(sessionId);
            } catch {
              // Session may already be finished
            }
          }
          await commands.deleteNote(noteId, !!sessionId);
          await loadTimeline();
        } catch (e) {
          console.error('Failed to delete note:', e);
          notifyError('Failed to delete note', e);
        } finally {
          clearItemDeleting('note', noteId);
        }
      },
    };
  }

  async function handleDeletePendingCommit(commitId: string, sessionId?: string) {
    markItemDeleting('commit', commitId);
    try {
      if (sessionId) {
        try {
          await commands.cancelSession(sessionId);
        } catch {
          // Session may already be finished
        }
      }
      await commands.deletePendingCommit(commitId, !!sessionId);
      await loadTimeline();
    } catch (e) {
      console.error('Failed to delete pending commit:', e);
      notifyError('Failed to delete pending commit', e);
    } finally {
      clearItemDeleting('commit', commitId);
    }
  }

  function handleDeleteReview(reviewId: string, sessionId?: string) {
    confirmDelete = {
      title: 'Delete Review',
      message:
        'Are you sure you want to delete this review and all its comments?' +
        (sessionId ? ' The linked session will also be deleted.' : ''),
      onConfirm: async () => {
        confirmDelete = null;
        markItemDeleting('review', reviewId);
        try {
          if (sessionId) {
            try {
              await commands.cancelSession(sessionId);
            } catch {
              // Session may already be finished
            }
          }
          await commands.deleteReview(reviewId, !!sessionId);
          await loadTimeline();
        } catch (e) {
          console.error('Failed to delete review:', e);
          notifyError('Failed to delete review', e);
        } finally {
          clearItemDeleting('review', reviewId);
        }
      },
    };
  }

  // =========================================================================
  // Display helpers
  // =========================================================================

  function extractWorkspaceBaseUrl(rawUri: string): string | null {
    try {
      const uri = new URL(rawUri);
      const segments = uri.pathname.split('/').filter(Boolean);
      const userSegmentIndex = segments.findIndex((segment) => segment.startsWith('@'));
      if (userSegmentIndex === -1 || userSegmentIndex + 1 >= segments.length) {
        return null;
      }
      return `${uri.origin}/${segments[userSegmentIndex]}/${segments[userSegmentIndex + 1]}`;
    } catch {
      return null;
    }
  }

  function resolveWorkspaceUrl(info: WorkspaceInfo): string | null {
    const uris = info['uris'];
    if (!Array.isArray(uris)) {
      return null;
    }

    for (const uri of uris) {
      if (typeof uri !== 'string') {
        continue;
      }
      const workspaceBaseUrl = extractWorkspaceBaseUrl(uri);
      if (workspaceBaseUrl) {
        return workspaceBaseUrl;
      }
    }

    return null;
  }

  async function loadWorkspaceUrl() {
    const requestId = ++workspaceInfoRequestId;
    try {
      const info = await commands.getWorkspaceInfo(branch.id);
      if (requestId !== workspaceInfoRequestId) {
        return;
      }
      workspaceUrl = resolveWorkspaceUrl(info);
    } catch (e) {
      if (requestId !== workspaceInfoRequestId) {
        return;
      }
      workspaceUrl = null;
      console.debug('Failed to resolve workspace URL:', e);
    }
  }

  function handleStatusBadgeClick() {
    if (status !== 'running' || !workspaceUrl) {
      return;
    }
    commands.openUrl(workspaceUrl).catch((e) => {
      console.error('Failed to open workspace URL:', e);
    });
  }

  function statusLabel(s: WorkspaceStatus | null): string {
    switch (s) {
      case 'starting':
        return 'Starting';
      case 'running':
        return 'Running';
      case 'stopped':
        return 'Stopped';
      case 'error':
        return 'Error';
      default:
        return 'Unknown';
    }
  }
</script>

<div class="branch-card remote" class:deleting data-branch-id={branch.id}>
  {#if deleting}
    <div class="deleting-overlay">
      <Spinner size={16} />
      <span>Deleting…</span>
    </div>
  {:else}
    <!-- Header -->
    <div class="card-header">
      <Cloud size={14} class="cloud-icon header-icon" />
      <BranchCardHeaderInfo
        branchName={branch.branchName}
        {repoLabel}
        secondaryLabel={branch.workspaceName}
      />
      <div class="header-actions">
        {#if status === 'running' && workspaceUrl}
          <button
            class="status-badge running clickable"
            onclick={handleStatusBadgeClick}
            type="button"
            title="Open workspace in browser"
          >
            <CircleCheck size={12} />
            <span>{statusLabel(status)}</span>
          </button>
        {:else}
          <div
            class="status-badge"
            class:starting={status === 'starting'}
            class:running={status === 'running'}
            class:stopped={status === 'stopped'}
            class:error={status === 'error'}
          >
            {#if status === 'starting'}
              <Spinner size={12} />
            {:else if status === 'running'}
              <CircleCheck size={12} />
            {:else if status === 'stopped'}
              <CirclePause size={12} />
            {:else if status === 'error'}
              <AlertCircle size={12} />
            {/if}
            <span>{statusLabel(status)}</span>
          </div>
        {/if}
        <DropdownMenu items={menuItems} />
      </div>
    </div>

    <!-- Content area — varies by status -->
    <div class="card-content">
      <ReasonBanner reason={repoLabel?.reason} onDismiss={handleDismissReason} />
      {#if status === 'starting'}
        <div class="status-view starting-view">
          <Spinner size={20} />
          <span class="status-text">Provisioning workspace…</span>
          {#if longProvisioning}
            <span class="status-hint"
              >Still provisioning. Large repositories can take several minutes. This view updates
              automatically when ready.</span
            >
          {:else}
            <span class="status-hint"
              >This can take a few minutes, depending on repository size.</span
            >
          {/if}
        </div>
      {:else if status === 'running'}
        <!-- Timeline UI (same pattern as BranchCard) -->
        {#if timelineLoading && !timeline}
          <div class="loading">
            <Spinner size={14} />
            <span>Loading...</span>
          </div>
        {:else if timelineError && !timeline}
          <div class="timeline-error">
            <span>{timelineError}</span>
          </div>
        {:else if timeline}
          <BranchTimeline
            {timeline}
            pendingItems={pendingTimelineItems}
            deletingItems={deletingTimelineItems}
            reviewCommentBreakdown={timelineReviewDetailsById}
            onSessionClick={handleTimelineSessionClick}
            onCommitClick={handleCommitClick}
            onNoteClick={handleNoteClick}
            onReviewClick={handleReviewClick}
            onDeletePendingCommit={handleDeletePendingCommit}
            onDeleteNote={handleDeleteNote}
            onDeleteReview={handleDeleteReview}
            onNewNote={() => openNewSession('note')}
            onNewCommit={() => openNewSession('commit')}
            onNewReview={hasCodeChanges ? () => openNewSession('review') : undefined}
            newSessionDisabled={showNewSession}
          >
            {#snippet footerActions()}
              {#if hasCodeChanges}
                <div class="footer-right-actions">
                  <button
                    class="diff-btn"
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
      {:else if status === 'stopped'}
        <div class="status-view stopped-view">
          <CirclePause size={20} />
          <span class="status-text">Workspace stopped</span>
          <span class="status-hint">Delete and recreate to start a new workspace</span>
        </div>
      {:else if status === 'error'}
        <div class="status-view error-view">
          <AlertCircle size={20} />
          <span class="status-text">Workspace error</span>
          {#if error}
            <span class="status-hint">{error}</span>
          {:else}
            <span class="status-hint">Something went wrong. Try deleting and recreating.</span>
          {/if}
          <button class="retry-btn" onclick={retryWorkspace} disabled={retrying}>
            {#if retrying}
              <Spinner size={12} />
              Retrying…
            {:else}
              Retry
            {/if}
          </button>
        </div>
      {:else}
        <div class="status-view">
          <span class="status-text">Unknown status</span>
        </div>
      {/if}
    </div>
  {/if}
</div>

{#if openNote}
  <NoteModal title={openNote.title} content={openNote.content} onClose={() => (openNote = null)} />
{/if}

{#if showNewSession}
  <NewSessionModal
    {branch}
    mode={newSessionMode}
    initialPrompt={draftPrompt}
    remote
    onClose={handleNewSessionClose}
    onStarted={handleNewSessionStarted}
  />
{/if}

{#if showBranchDiff}
  <DiffModal
    branchId={branch.id}
    commitSha={reviewDiffTarget?.commitSha}
    scope={reviewDiffTarget?.scope ?? 'branch'}
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

{#if openSessionId}
  <SessionModal
    sessionId={openSessionId}
    onClose={() => {
      openSessionId = null;
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
    transition: border-color 0.15s ease;
  }

  .branch-card:hover:not(.deleting) {
    border-color: var(--border-muted);
  }

  .branch-card.deleting {
    opacity: 0.6;
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
    border-bottom: 1px solid var(--border-subtle);
  }

  :global(.cloud-icon) {
    color: var(--ui-accent);
    flex-shrink: 0;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .diff-btn {
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

  .diff-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-muted);
    background: var(--bg-hover);
  }

  .diff-btn :global(svg) {
    flex-shrink: 0;
  }

  /* Status badge */
  .status-badge {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 3px 10px;
    border-radius: 12px;
    font-size: var(--size-xs);
    font-weight: 500;
    white-space: nowrap;
  }

  .status-badge.clickable {
    appearance: none;
    border: none;
    cursor: pointer;
    font: inherit;
  }

  .status-badge.clickable:focus-visible {
    outline: 2px solid var(--border-emphasis);
    outline-offset: 2px;
  }

  .status-badge.starting {
    background-color: rgba(210, 153, 34, 0.1);
    color: rgb(210, 153, 34);
  }

  .status-badge.running {
    background-color: rgba(63, 185, 80, 0.1);
    color: var(--ui-accent);
  }

  .status-badge.stopped {
    background-color: rgba(139, 148, 158, 0.1);
    color: var(--text-muted);
  }

  .status-badge.error {
    background-color: rgba(248, 81, 73, 0.1);
    color: var(--ui-danger);
  }

  /* Content */
  .card-content {
    display: flex;
    flex-direction: column;
    padding: 16px;
    min-height: 80px;
  }

  /* Timeline loading / error */
  .loading {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .timeline-error {
    color: var(--ui-danger);
    font-size: var(--size-sm);
  }

  .footer-right-actions {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  /* Status views (starting, stopped, error) */
  .status-view {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 32px 16px;
    text-align: center;
  }

  .status-view :global(svg) {
    color: var(--text-faint);
  }

  .starting-view :global(svg) {
    color: rgb(210, 153, 34);
  }

  .error-view :global(svg) {
    color: var(--ui-danger);
  }

  .status-text {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .status-hint {
    font-size: var(--size-xs);
    color: var(--text-muted);
    max-width: 280px;
  }

  .retry-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
    padding: 6px 16px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition:
      border-color 0.15s,
      background-color 0.15s;
  }

  .retry-btn:hover:not(:disabled) {
    border-color: var(--border-emphasis);
    background: var(--bg-hover);
  }

  .retry-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
