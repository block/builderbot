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
    WorkspaceStatus,
  } from '../../types';
  import * as commands from '../../commands';
  import BranchTimeline from '../timeline/BranchTimeline.svelte';
  import DropdownMenu, { type MenuItem } from '../../shared/DropdownMenu.svelte';
  import DiffModal from '../diff/DiffModal.svelte';
  import SessionModal from '../sessions/SessionModal.svelte';
  import NewSessionModal from '../sessions/NewSessionModal.svelte';
  import NoteModal from '../notes/NoteModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import BranchCardHeaderInfo from './BranchCardHeaderInfo.svelte';
  import { formatBaseBranch } from './branchCardHelpers';
  import { alerts } from '../../shared/alerts.svelte';
  import { projectStateStore } from '../../stores/projectState.svelte';

  interface Props {
    branch: Branch;
    repoLabel?: string | null;
    deleting?: boolean;
    onDelete?: () => void;
    onRename?: (branchName: string) => void;
    onWorkspaceStatusChange?: (status: WorkspaceStatus) => void;
  }

  let {
    branch,
    repoLabel = null,
    deleting = false,
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
  const POLL_TIMEOUT_MS = 5 * 60 * 1000; // 5 minutes

  // Error state
  let error = $state<string | null>(null);

  // Timeline state
  let timeline = $state<BranchTimelineData | null>(null);
  let timelineLoading = $state(true);
  let timelineError = $state<string | null>(null);

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
  // Status polling
  // =========================================================================

  onMount(() => {
    if (status === 'starting') {
      startPolling();
    }
    if (status === 'running') {
      loadTimeline();
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
    pollTimer = setInterval(async () => {
      // Safety valve: stop polling after timeout to avoid infinite loops
      // when a workspace never materializes.
      if (pollStartedAt && Date.now() - pollStartedAt > POLL_TIMEOUT_MS) {
        console.error('Workspace polling timed out after 5 minutes');
        polledStatus = 'error';
        onWorkspaceStatusChange?.('error');
        error = 'Workspace provisioning timed out';
        stopPolling();
        return;
      }

      try {
        const newStatus = (await commands.pollWorkspaceStatus(branch.id)) as WorkspaceStatus;
        polledStatus = newStatus;
        onWorkspaceStatusChange?.(newStatus);
        if (newStatus === 'running') {
          stopPolling();
          loadTimeline();
        } else if (newStatus !== 'starting') {
          stopPolling();
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
        // when the first poll fires. The backend tolerates this when the
        // DB status is Starting, but as a safety net we also keep polling
        // on the frontend side if our local status is still 'starting'.
        if (status === 'starting') {
          console.debug('Poll failed while starting (workspace may not exist yet), retrying…', e);
        } else {
          console.error('Failed to poll workspace status:', e);
          polledStatus = 'error';
          onWorkspaceStatusChange?.('error');
          error = msg;
          stopPolling();
        }
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

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  let retrying = $state(false);

  async function retryWorkspace() {
    retrying = true;
    error = null;
    polledStatus = 'starting';

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
        loadTimeline();
      }
    });
  }

  async function loadTimeline() {
    timelineLoading = true;
    timelineError = null;
    try {
      timeline = await commands.getBranchTimeline(branch.id);
    } catch (e) {
      timelineError = e instanceof Error ? e.message : String(e);
    } finally {
      timelineLoading = false;
    }
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
    console.info('[RemoteBranchCard] new session started:', {
      sessionId: result.sessionId,
      projectId: branch.projectId,
      branchId: branch.id,
    });
    // Track the running session in the project state store
    projectStateStore.addRunningSession(branch.projectId, result.sessionId);
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

  function handleReviewClick(_reviewId: string) {
    showBranchDiff = true;
  }

  function handleCommitClick(sha: string) {
    commitDiffSha = sha;
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

  async function handleDeletePendingCommit(commitId: string, sessionId?: string) {
    try {
      if (sessionId) {
        try {
          await commands.cancelSession(sessionId);
        } catch {
          // Session may already be finished
        }
      }
      await commands.deletePendingCommit(commitId, !!sessionId);
      loadTimeline();
    } catch (e) {
      console.error('Failed to delete pending commit:', e);
      notifyError('Failed to delete pending commit', e);
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

  // =========================================================================
  // Display helpers
  // =========================================================================

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
        <DropdownMenu items={menuItems} />
      </div>
    </div>

    <!-- Content area — varies by status -->
    <div class="card-content">
      {#if status === 'starting'}
        <div class="status-view starting-view">
          <Spinner size={20} />
          <span class="status-text">Provisioning workspace…</span>
          <span class="status-hint">This usually takes 30–60 seconds</span>
        </div>
      {:else if status === 'running'}
        <!-- Timeline UI (same pattern as BranchCard) -->
        {#if timelineLoading}
          <div class="loading">
            <Spinner size={14} />
            <span>Loading...</span>
          </div>
        {:else if timelineError}
          <div class="timeline-error">
            <span>{timelineError}</span>
          </div>
        {:else if timeline}
          <BranchTimeline
            {timeline}
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
                    onclick={() => (showBranchDiff = true)}
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
    scope="branch"
    beforeLabel={formatBaseBranch(branch.baseBranch)}
    afterLabel={branch.branchName}
    onClose={() => {
      showBranchDiff = false;
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
      loadTimeline();
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
