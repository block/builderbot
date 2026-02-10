<!--
  BranchCard.svelte - Card display for a tracked branch

  Shows branch name, base branch, and a unified timeline of commits/notes/reviews.
  Footer has a single "New" button: click for commit, long-press to pick note/commit.
  Opens a modal for prompt entry; draft text is preserved across open/close.

  Timeline items are clickable:
  - Commits open a limited diff view (no commenting / reference files)
  - Notes open a markdown viewer
  - Each item shows session + delete actions on hover
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import {
    GitBranch,
    GitCommitHorizontal,
    Loader2,
    Trash2,
    FileDiff,
    StickyNote,
    Plus,
    Copy,
  } from 'lucide-svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import type { Branch, BranchTimeline as BranchTimelineData, BranchSessionType } from './types';
  import * as commands from './commands';
  import BranchTimeline from './BranchTimeline.svelte';
  import DropdownMenu, { type MenuItem } from './DropdownMenu.svelte';
  import DiffModal from './DiffModal.svelte';
  import SessionModal from './SessionModal.svelte';
  import NewSessionModal from './NewSessionModal.svelte';
  import NoteModal from './NoteModal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';

  interface Props {
    branch: Branch;
    deleting?: boolean;
    onDelete?: () => void;
  }

  let { branch, deleting = false, onDelete }: Props = $props();

  async function copyWorktreePath() {
    const path = branch.worktreePath;
    if (path) {
      try {
        await navigator.clipboard.writeText(path);
      } catch {
        // clipboard API may fail in some contexts
      }
    }
  }

  const menuItems: MenuItem[] = $derived([
    ...(branch.worktreePath
      ? [{ label: 'Copy Worktree Path', icon: Copy, action: copyWorktreePath }]
      : []),
    { label: 'Delete Branch', icon: Trash2, danger: true, action: () => onDelete?.() },
  ]);

  let timeline = $state<BranchTimelineData | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let showBranchDiff = $state(false);

  // Commit diff modal (opened by clicking a commit in the timeline)
  let commitDiffSha = $state<string | null>(null);

  // Note modal (opened by clicking a note in the timeline)
  let openNote = $state<{ title: string; content: string } | null>(null);

  // New session modal state
  let showNewSession = $state(false);
  let newSessionMode = $state<BranchSessionType>('commit');
  let draftPrompt = $state('');

  // Long-press picker state
  let longPressTimer: ReturnType<typeof setTimeout> | null = null;
  let showPicker = $state(false);
  let pickerRef = $state<HTMLDivElement | null>(null);
  let didLongPress = false;

  // Session modal (opened after starting a branch session, or from timeline)
  let openSessionId = $state<string | null>(null);

  // Confirm delete dialog
  let confirmDelete = $state<{
    title: string;
    message: string;
    onConfirm: () => void;
  } | null>(null);

  // Listen for session completion to refresh timeline
  let unlistenStatus: UnlistenFn | null = null;

  onMount(() => {
    loadTimeline();
    listenForStatusChanges();
  });

  onDestroy(() => {
    unlistenStatus?.();
  });

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
    loading = true;
    error = null;
    try {
      timeline = await commands.getBranchTimeline(branch.id);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function formatBaseBranch(baseBranch: string): string {
    return baseBranch.replace(/^origin\//, '');
  }

  // =========================================================================
  // New session modal
  // =========================================================================

  function openNewSession(mode: BranchSessionType) {
    newSessionMode = mode;
    showNewSession = true;
    showPicker = false;
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
  // Long-press button logic
  // =========================================================================

  function handlePointerDown() {
    didLongPress = false;
    longPressTimer = setTimeout(() => {
      didLongPress = true;
      showPicker = true;
    }, 400);
  }

  function handlePointerUp() {
    if (longPressTimer) {
      clearTimeout(longPressTimer);
      longPressTimer = null;
    }
    if (!didLongPress && !showPicker) {
      openNewSession('commit');
    }
  }

  function handlePointerLeave() {
    if (longPressTimer) {
      clearTimeout(longPressTimer);
      longPressTimer = null;
    }
  }

  function handlePickerClickOutside(e: MouseEvent) {
    if (showPicker && pickerRef && !pickerRef.contains(e.target as Node)) {
      showPicker = false;
    }
  }

  function handlePickerKeydown(e: KeyboardEvent) {
    if (showPicker && e.key === 'Escape') {
      showPicker = false;
      e.stopPropagation();
    }
  }

  // =========================================================================
  // Timeline item interactions
  // =========================================================================

  function handleTimelineSessionClick(sessionId: string) {
    openSessionId = sessionId;
  }

  function handleCommitClick(sha: string) {
    commitDiffSha = sha;
  }

  function handleNoteClick(_noteId: string, title: string, content: string) {
    openNote = { title, content };
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
        }
      },
    };
  }

  async function handleDeletePendingCommit(commitId: string, sessionId?: string) {
    // For pending/failed commits, cancel the session if running, then delete the DB record.
    // No confirmation needed — these artifacts were never finalized.
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
    }
  }
</script>

<svelte:window onclick={handlePickerClickOutside} onkeydown={handlePickerKeydown} />

<div class="branch-card" class:deleting>
  {#if deleting}
    <div class="deleting-overlay">
      <Loader2 size={16} class="spinner" />
      <span>Deleting…</span>
    </div>
  {:else}
    <div class="card-header">
      <div class="branch-info">
        <GitBranch size={16} class="branch-icon" />
        <span class="branch-name">{branch.branchName}</span>
        <span class="branch-separator">›</span>
        <span class="base-branch-name">{formatBaseBranch(branch.baseBranch)}</span>
      </div>
      <div class="header-actions">
        <button class="view-diff-btn" onclick={() => (showBranchDiff = true)} title="View diff">
          <FileDiff size={16} />
        </button>
        <DropdownMenu items={menuItems} />
      </div>
    </div>

    <div class="card-content">
      {#if loading}
        <div class="loading">
          <Loader2 size={14} class="spinner" />
          <span>Loading...</span>
        </div>
      {:else if error}
        <div class="error">
          <span>{error}</span>
        </div>
      {:else if timeline}
        <BranchTimeline
          {timeline}
          onSessionClick={handleTimelineSessionClick}
          onCommitClick={handleCommitClick}
          onNoteClick={handleNoteClick}
          onDeleteCommit={handleDeleteCommit}
          onDeletePendingCommit={handleDeletePendingCommit}
          onDeleteNote={handleDeleteNote}
        />
      {/if}
    </div>

    <!-- Footer with single action button -->
    <div class="card-footer">
      <div class="new-btn-container" bind:this={pickerRef}>
        <button
          class="new-btn"
          onpointerdown={handlePointerDown}
          onpointerup={handlePointerUp}
          onpointerleave={handlePointerLeave}
          disabled={showNewSession}
          title="New commit (hold for options)"
        >
          <Plus size={14} />
        </button>
        {#if showPicker}
          <div class="picker-dropdown">
            <button class="picker-item" onclick={() => openNewSession('commit')}>
              <GitCommitHorizontal size={14} />
              <span>Commit</span>
            </button>
            <button class="picker-item" onclick={() => openNewSession('note')}>
              <StickyNote size={14} />
              <span>Note</span>
            </button>
          </div>
        {/if}
      </div>
    </div>
  {/if}
</div>

{#if showBranchDiff}
  <DiffModal
    branchId={branch.id}
    scope="branch"
    beforeLabel={formatBaseBranch(branch.baseBranch)}
    afterLabel={branch.branchName}
    onClose={() => (showBranchDiff = false)}
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
    onClose={() => (commitDiffSha = null)}
  />
{/if}

{#if openNote}
  <NoteModal title={openNote.title} content={openNote.content} onClose={() => (openNote = null)} />
{/if}

{#if showNewSession}
  <NewSessionModal
    {branch}
    mode={newSessionMode}
    initialPrompt={draftPrompt}
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
    padding: 14px 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .branch-info {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .view-diff-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 6px;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .view-diff-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  :global(.branch-icon) {
    color: var(--status-renamed);
    flex-shrink: 0;
  }

  .branch-name {
    font-size: var(--size-md);
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: -0.01em;
  }

  .branch-separator {
    color: var(--text-faint);
    font-size: var(--size-md);
    margin: 0 2px;
  }

  .base-branch-name {
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-muted);
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

  /* Footer */
  .card-footer {
    display: flex;
    justify-content: flex-end;
    padding: 6px 12px;
  }

  /* Single "New" button with long-press picker */
  .new-btn-container {
    position: relative;
  }

  .new-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    padding: 0;
    background: none;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s;
    user-select: none;
    -webkit-user-select: none;
    touch-action: none;
  }

  .new-btn:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--border-muted);
    background: var(--bg-hover);
  }

  .new-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  /* Long-press picker dropdown */
  .picker-dropdown {
    position: absolute;
    bottom: calc(100% + 4px);
    right: 0;
    background: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow:
      0 4px 12px rgba(0, 0, 0, 0.12),
      0 1px 4px rgba(0, 0, 0, 0.08);
    overflow: hidden;
    z-index: 100;
    min-width: 120px;
    padding: 4px 0;
  }

  .picker-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 7px 12px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: background-color 0.1s;
    text-align: left;
    white-space: nowrap;
  }

  .picker-item:hover {
    background: var(--bg-hover);
  }

  .picker-item :global(svg) {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  :global(.spinner) {
    animation: spin 1s linear infinite;
    flex-shrink: 0;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
