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
  import { onMount } from 'svelte';
  import {
    GitBranch,
    GitCommitHorizontal,
    Loader2,
    Trash2,
    FileDiff,
    StickyNote,
    Plus,
    Copy,
    Play,
    Hammer,
    FlaskConical,
    Sparkles,
    CheckCircle2,
    Wrench,
    AlertCircle,
    StopCircle,
    CheckCircle,
    ChevronDown,
    Zap,
    Wand2,
    MoreVertical,
  } from 'lucide-svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import type { Branch, BranchTimeline as BranchTimelineData, BranchSessionType } from './types';
  import * as commands from './commands';
  import type { ProjectAction } from './commands';
  import BranchTimeline from './BranchTimeline.svelte';
  import DiffModal from './DiffModal.svelte';
  import SessionModal from './SessionModal.svelte';
  import NewSessionModal from './NewSessionModal.svelte';
  import NoteModal from './NoteModal.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import ActionOutputModal from './ActionOutputModal.svelte';
  import { runBranchAction, type ActionStatusEvent } from './services/actions';

  interface Props {
    branch: Branch;
    deleting?: boolean;
    onDelete?: () => void;
  }

  let { branch, deleting = false, onDelete }: Props = $props();

  async function copyWorktreePath() {
    showMoreMenu = false;
    const path = branch.worktreePath;
    if (path) {
      try {
        await navigator.clipboard.writeText(path);
      } catch {
        // clipboard API may fail in some contexts
      }
    }
  }

  // Dropdown state
  let showMoreMenu = $state(false);
  let showActionsSubmenu = $state(false);
  let actionsSubmenuTimeout = $state<ReturnType<typeof setTimeout> | null>(null);

  let timeline = $state<BranchTimelineData | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let showBranchDiff = $state(false);

  // Actions state
  let actions = $state<ProjectAction[]>([]);

  type RunningAction = {
    executionId: string;
    actionId: string;
    actionName: string;
    status: 'running' | 'completed' | 'failed' | 'stopped';
    exitCode?: number | null;
    startedAt?: number;
    completedAt?: number | null;
    fading?: boolean;
  };
  let runningActions = $state<RunningAction[]>([]);
  let actionOutputModal = $state<{ executionId: string; actionName: string } | null>(null);

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
  let unlistenActionStatus: UnlistenFn | null = null;

  // Set up event listeners immediately (synchronously) at module level like old codebase
  $effect(() => {
    const branchId = branch.id;
    const branchName = branch.branchName;
    console.log('[BranchCard] Setting up listeners for branch:', branchId, branchName);

    listen<{
      sessionId: string;
      status: string;
    }>('session-status-changed', (event) => {
      const { status } = event.payload;
      if (status === 'completed' || status === 'error' || status === 'cancelled') {
        loadTimeline();
      }
    }).then((unlisten) => {
      unlistenStatus = unlisten;
      console.log('[BranchCard] Session status listener registered for:', branchId);
    });

    listen<ActionStatusEvent>('action_status', (event) => {
      const payload = event.payload;
      console.log('[BranchCard] Received action_status event:', payload);

      // Only process events for this branch
      if (payload.branchId !== branchId) {
        console.log(
          '[BranchCard] Ignoring event for different branch:',
          payload.branchId,
          'vs',
          branchId
        );
        return;
      }

      console.log(
        '[BranchCard] Processing action_status for branch:',
        branchId,
        'status:',
        payload.status
      );

      const existingIndex = runningActions.findIndex((a) => a.executionId === payload.executionId);

      if (payload.status === 'running') {
        if (existingIndex === -1) {
          console.log('[BranchCard] Adding running action:', payload.actionName);
          runningActions.push({
            executionId: payload.executionId,
            actionId: payload.actionId,
            actionName: payload.actionName,
            status: 'running',
            startedAt: payload.startedAt ?? Date.now(),
          });
          console.log(
            '[BranchCard] runningActions now:',
            runningActions.length,
            runningActions.map((a) => a.actionName)
          );
        }
      } else {
        // Action completed/failed/stopped - update status
        if (existingIndex !== -1) {
          runningActions[existingIndex].status = payload.status as any;
          runningActions[existingIndex].exitCode = payload.exitCode;
          runningActions[existingIndex].completedAt = payload.completedAt;

          // Auto-remove successful completions (with fade for secondary, instant for primary)
          if (payload.status === 'completed') {
            const action = runningActions[existingIndex];
            const isPrimaryAction = primaryRunAction && action.actionId === primaryRunAction.id;

            setTimeout(
              () => {
                const foundAction = runningActions.find(
                  (a) => a.executionId === payload.executionId
                );
                if (foundAction && !isPrimaryAction) {
                  // Secondary actions fade out
                  foundAction.fading = true;
                }
                // Remove after animation completes (or immediately for primary)
                setTimeout(
                  () => {
                    runningActions = runningActions.filter(
                      (a) => a.executionId !== payload.executionId
                    );
                  },
                  isPrimaryAction ? 0 : 300
                ); // Match CSS transition duration for secondary
              },
              isPrimaryAction ? 1000 : 2000
            ); // Shorter display time for primary action
          }
        }
      }
    }).then((unlisten) => {
      unlistenActionStatus = unlisten;
      console.log('[BranchCard] Action status listener registered for:', branchId);
    });

    return () => {
      unlistenStatus?.();
      unlistenActionStatus?.();
    };
  });

  onMount(() => {
    loadTimeline();
    loadActions();
  });

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

  async function loadActions() {
    try {
      // Load actions for this branch's project
      actions = await commands.listProjectActions(branch.projectId);
    } catch (e) {
      console.error('Failed to load actions:', e);
      actions = [];
    }
  }

  // Group actions by type
  let groupedActions = $derived.by(() => {
    const groups: Record<string, ProjectAction[]> = {
      prerun: [],
      run: [],
      build: [],
      format: [],
      check: [],
      test: [],
      cleanUp: [],
    };
    for (const action of actions) {
      if (groups[action.actionType]) {
        groups[action.actionType].push(action);
      }
    }
    return groups;
  });

  // Get the primary run action (first run action)
  let primaryRunAction = $derived.by(() => {
    return groupedActions.run[0] ?? null;
  });

  // Get remaining run actions (excluding the primary one)
  let remainingRunActions = $derived.by(() => {
    return groupedActions.run.slice(1);
  });

  // Actions submenu handlers
  function handleActionsSubmenuEnter() {
    if (actionsSubmenuTimeout) {
      clearTimeout(actionsSubmenuTimeout);
      actionsSubmenuTimeout = null;
    }
    showActionsSubmenu = true;
  }

  function handleActionsSubmenuLeave() {
    actionsSubmenuTimeout = setTimeout(() => {
      showActionsSubmenu = false;
      actionsSubmenuTimeout = null;
    }, 100);
  }

  // Track the primary action's execution status
  let primaryActionExecution = $derived.by(() => {
    if (!primaryRunAction) return null;
    return runningActions.find((a) => a.actionId === primaryRunAction.id) ?? null;
  });

  // Filter running actions to exclude the primary action
  let secondaryRunningActions = $derived.by(() => {
    if (!primaryRunAction) return runningActions;
    return runningActions.filter((a) => a.actionId !== primaryRunAction.id);
  });

  async function handleRunAction(action: ProjectAction) {
    showMoreMenu = false;

    // Check if this action is already running
    const existingExecution = runningActions.find((a) => a.actionId === action.id);

    if (existingExecution) {
      // Action already running, open modal to view output
      actionOutputModal = {
        executionId: existingExecution.executionId,
        actionName: action.name,
      };
      return;
    }

    // Start the action silently (don't open modal)
    try {
      const executionId = await runBranchAction(branch.id, action.id);
      // The running action will be added via the event listener
      // Don't auto-show output modal - user can click to view
    } catch (e) {
      console.error('Failed to run action:', e);
      error = e instanceof Error ? e.message : String(e);
    }
  }

  // Handle showing action output
  function handleShowActionOutput(execution: RunningAction) {
    actionOutputModal = {
      executionId: execution.executionId,
      actionName: execution.actionName,
    };
  }

  // Close dropdowns when clicking outside
  function handleClickOutside(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (!target.closest('.more-menu-container')) {
      showMoreMenu = false;
    }
  }

  function toggleMoreMenu(e: MouseEvent) {
    e.stopPropagation();
    showMoreMenu = !showMoreMenu;
  }

  function handleDeleteFromMenu() {
    showMoreMenu = false;
    onDelete?.();
  }

  function getActionIcon(actionType: string) {
    switch (actionType) {
      case 'prerun':
        return Zap;
      case 'run':
        return Play;
      case 'build':
        return Hammer;
      case 'format':
        return Wand2;
      case 'check':
        return CheckCircle;
      case 'test':
        return FlaskConical;
      case 'cleanUp':
        return Wrench;
      default:
        return Wrench;
    }
  }

  function getActionTypeLabel(actionType: string): string {
    switch (actionType) {
      case 'prerun':
        return 'Prerun';
      case 'run':
        return 'Run';
      case 'build':
        return 'Build';
      case 'format':
        return 'Format';
      case 'check':
        return 'Check';
      case 'test':
        return 'Test';
      case 'cleanUp':
        return 'Clean Up';
      default:
        return 'Action';
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

<svelte:window
  onclick={(e) => {
    handlePickerClickOutside(e);
    handleClickOutside(e);
  }}
  onkeydown={handlePickerKeydown}
/>

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
        <!-- Running actions (excluding primary action) -->
        {#each secondaryRunningActions as execution (execution.executionId)}
          <div class="running-action-container" class:fading={execution.fading}>
            <button
              class="running-action-button"
              class:completed={execution.status === 'completed'}
              class:failed={execution.status === 'failed'}
              onclick={() => handleShowActionOutput(execution)}
              title="View output"
            >
              {#if execution.status === 'running'}
                <Loader2 size={12} class="spinner" />
              {:else if execution.status === 'completed'}
                <CheckCircle size={12} />
              {:else if execution.status === 'failed'}
                <AlertCircle size={12} />
              {:else}
                <StopCircle size={12} />
              {/if}
              {execution.actionName}
            </button>
          </div>
        {/each}
        <!-- Primary run action button -->
        {#if primaryRunAction && branch.branchType === 'local'}
          <div class="primary-action-container">
            <button
              class="primary-action-button"
              class:running={primaryActionExecution?.status === 'running'}
              class:completed={primaryActionExecution?.status === 'completed'}
              class:failed={primaryActionExecution?.status === 'failed'}
              onclick={() =>
                primaryActionExecution
                  ? handleShowActionOutput(primaryActionExecution)
                  : handleRunAction(primaryRunAction)}
              title={primaryActionExecution ? 'View output' : `Run ${primaryRunAction.name}`}
            >
              {#if primaryActionExecution?.status === 'running'}
                <Loader2 size={13} class="spinner" />
              {:else if primaryActionExecution?.status === 'completed'}
                <CheckCircle size={13} />
              {:else if primaryActionExecution?.status === 'failed'}
                <AlertCircle size={13} />
              {:else}
                <Play size={13} />
              {/if}
              {primaryRunAction.name}
            </button>
          </div>
        {/if}
        <button class="view-diff-btn" onclick={() => (showBranchDiff = true)} title="View diff">
          <FileDiff size={16} />
        </button>
        <div class="more-menu-container">
          <button class="more-button" onclick={toggleMoreMenu} title="More options">
            <MoreVertical size={16} />
          </button>
          {#if showMoreMenu}
            <div class="more-menu">
              <!-- Actions submenu -->
              {#if actions.length > 0 && branch.branchType === 'local'}
                <div class="submenu-container">
                  <button
                    class="more-menu-item submenu-trigger"
                    onmouseenter={handleActionsSubmenuEnter}
                    onmouseleave={handleActionsSubmenuLeave}
                  >
                    <Play size={14} />
                    Actions
                    <ChevronDown size={12} class="submenu-chevron" />
                  </button>
                  {#if showActionsSubmenu}
                    <div
                      class="submenu"
                      role="group"
                      onmouseenter={handleActionsSubmenuEnter}
                      onmouseleave={handleActionsSubmenuLeave}
                    >
                      <!-- Actions in order: Run, Build, Format, Check, Test, CleanUp, Prerun -->
                      {#each ['run', 'build', 'format', 'check', 'test', 'cleanUp', 'prerun'] as type}
                        {@const typeActions =
                          type === 'run' ? remainingRunActions : groupedActions[type]}
                        {#if typeActions.length > 0}
                          <!-- All actions shown directly -->
                          {#each typeActions as action (action.id)}
                            {@const Icon = getActionIcon(type)}
                            <button
                              class="more-menu-item action-item"
                              onclick={() => handleRunAction(action)}
                            >
                              <Icon size={14} />
                              {action.name}
                            </button>
                          {/each}
                        {/if}
                      {/each}
                    </div>
                  {/if}
                </div>
              {/if}

              <!-- Copy Worktree Path if available -->
              {#if branch.worktreePath}
                <div class="menu-separator"></div>
                <button class="more-menu-item" onclick={copyWorktreePath}>
                  <Copy size={14} />
                  Copy Worktree Path
                </button>
              {/if}

              <!-- Delete last -->
              <div class="menu-separator"></div>
              <button class="more-menu-item danger" onclick={handleDeleteFromMenu}>
                <Trash2 size={14} />
                Delete
              </button>
            </div>
          {/if}
        </div>
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

{#if actionOutputModal}
  <ActionOutputModal
    executionId={actionOutputModal.executionId}
    actionName={actionOutputModal.actionName}
    onClose={() => (actionOutputModal = null)}
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

  /* More menu */
  .more-menu-container {
    position: relative;
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

  .more-menu {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    overflow: visible;
    z-index: 100;
    min-width: 160px;
  }

  .more-menu-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 14px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
    text-align: left;
  }

  .more-menu-item:hover {
    background-color: var(--bg-hover);
  }

  .more-menu-item :global(svg) {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .more-menu-item.danger:hover {
    background-color: var(--ui-danger-bg);
    color: var(--ui-danger);
  }

  .more-menu-item.danger:hover :global(svg) {
    color: var(--ui-danger);
  }

  .menu-separator {
    height: 1px;
    background-color: var(--border-subtle);
    margin: 4px 0;
  }

  /* Submenu styles */
  .submenu-container {
    position: relative;
  }

  .submenu-trigger {
    justify-content: space-between;
  }

  .submenu-trigger :global(.submenu-chevron) {
    margin-left: auto;
    transform: rotate(-90deg);
  }

  .submenu {
    position: absolute;
    left: 100%;
    top: 0;
    margin-left: 2px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    max-height: 400px;
    z-index: 101;
    min-width: 160px;
    display: flex;
    flex-direction: column;
  }

  /* Enable scrolling for actions submenu */
  .more-menu > .submenu-container > .submenu {
    overflow-y: auto;
    overflow-x: visible;
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

  /* Primary action button */
  .primary-action-container {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .primary-action-button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px;
    background: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    white-space: nowrap;
  }

  .primary-action-button:hover {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
    background-color: var(--bg-hover);
  }

  .primary-action-button.running {
    background: var(--bg-elevated);
    border-color: var(--border-muted);
    color: var(--text-primary);
  }

  .primary-action-button.completed {
    border-color: var(--ui-success);
    color: var(--ui-success);
  }

  .primary-action-button.failed {
    border-color: var(--ui-danger);
    color: var(--ui-danger);
  }

  .primary-action-button :global(svg) {
    flex-shrink: 0;
    width: 13px;
    height: 13px;
  }

  /* Running actions */
  .running-action-container {
    display: flex;
    align-items: center;
    gap: 4px;
    opacity: 1;
    transition:
      opacity 0.3s ease,
      transform 0.3s ease;
  }

  .running-action-container.fading {
    opacity: 0;
    transform: scale(0.95);
  }

  .running-action-button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease;
  }

  .running-action-button:hover {
    background: var(--bg-hover);
    border-color: var(--border-focus);
  }

  .running-action-button.completed {
    border-color: var(--ui-success);
    color: var(--ui-success);
  }

  .running-action-button.failed {
    border-color: var(--ui-danger);
    color: var(--ui-danger);
  }

  .running-action-button :global(svg) {
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
