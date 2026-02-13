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
    GitCommitHorizontal,
    StickyNote,
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
  import SessionModal from '../sessions/SessionModal.svelte';
  import NewSessionModal from '../sessions/NewSessionModal.svelte';
  import NoteModal from '../notes/NoteModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';

  interface Props {
    branch: Branch;
    deleting?: boolean;
    onDelete?: () => void;
  }

  let { branch, deleting = false, onDelete }: Props = $props();

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

  // Confirm delete dialog
  let confirmDelete = $state<{
    title: string;
    message: string;
    onConfirm: () => void;
  } | null>(null);

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
      label: 'Delete Branch',
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
        error = 'Workspace provisioning timed out';
        stopPolling();
        return;
      }

      try {
        const newStatus = (await commands.pollWorkspaceStatus(branch.id)) as WorkspaceStatus;
        polledStatus = newStatus;
        if (newStatus === 'running') {
          stopPolling();
          loadTimeline();
        } else if (newStatus !== 'starting') {
          stopPolling();
        }
      } catch (e) {
        // During initial creation, `blox ws start` may still be running
        // when the first poll fires. The backend tolerates this when the
        // DB status is Starting, but as a safety net we also keep polling
        // on the frontend side if our local status is still 'starting'.
        if (status === 'starting') {
          console.debug('Poll failed while starting (workspace may not exist yet), retrying…', e);
        } else {
          console.error('Failed to poll workspace status:', e);
          polledStatus = 'error';
          error = e instanceof Error ? e.message : String(e);
          stopPolling();
        }
      }
    }, 3000);
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
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
      const { status } = event.payload;
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

  function handleNewSessionStarted(_result: { sessionId: string; artifactId: string }) {
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
    }
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
      <div class="branch-info">
        <Cloud size={16} class="cloud-icon" />
        <span class="branch-name">{branch.branchName}</span>
      </div>
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

    <!-- Subheader: workspace info -->
    {#if branch.workspaceName}
      <div class="card-subheader">
        <span class="workspace-name">{branch.workspaceName}</span>
      </div>
    {/if}

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
            onNoteClick={handleNoteClick}
            onDeletePendingCommit={handleDeletePendingCommit}
            onDeleteNote={handleDeleteNote}
            onNewNote={() => openNewSession('note')}
            onNewCommit={() => openNewSession('commit')}
            newSessionDisabled={showNewSession}
          />
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
        </div>
      {:else}
        <div class="status-view">
          <span class="status-text">Unknown status</span>
        </div>
      {/if}
    </div>

    <!-- Footer with separate note and commit buttons (only when running) -->
    {#if status === 'running'}
      <div class="card-footer">
        <div class="new-btn-group">
          <button
            class="new-item-btn"
            onclick={() => openNewSession('note')}
            disabled={showNewSession}
            title="New note"
          >
            <StickyNote size={13} />
            <span>New note</span>
          </button>
          <button
            class="new-item-btn"
            onclick={() => openNewSession('commit')}
            disabled={showNewSession}
            title="New commit"
          >
            <GitCommitHorizontal size={13} />
            <span>New commit</span>
          </button>
        </div>
      </div>
    {/if}
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
    justify-content: space-between;
    padding: 14px 16px 0;
  }

  .branch-info {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  :global(.cloud-icon) {
    color: var(--ui-accent);
    flex-shrink: 0;
  }

  .branch-name {
    font-size: var(--size-md);
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: -0.01em;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 8px;
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

  /* Subheader */
  .card-subheader {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 16px 12px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .workspace-name {
    font-size: var(--size-xs);
    color: var(--text-faint);
    font-family: 'SF Mono', 'Menlo', monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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

  /* Footer */
  .card-footer {
    display: flex;
    justify-content: flex-end;
    padding: 6px 12px;
  }

  /* Footer with separate note/commit buttons */
  .new-btn-group {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .new-item-btn {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 4px 10px;
    background: none;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    color: var(--text-faint);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s;
    white-space: nowrap;
  }

  .new-item-btn:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--border-muted);
    background: var(--bg-hover);
  }

  .new-item-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .new-item-btn :global(svg) {
    flex-shrink: 0;
  }

  :global(.spinner) {
    animation: spin 1s linear infinite;
    flex-shrink: 0;
  }
</style>
