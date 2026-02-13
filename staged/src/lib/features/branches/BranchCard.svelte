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
  import { onMount, onDestroy } from 'svelte';
  import {
    GitBranch,
    GitCommitHorizontal,
    GitPullRequestCreateArrow,
    GitPullRequestArrow,
    Trash2,
    FileDiff,
    FileText,
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
    ExternalLink,
  } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import type {
    Branch,
    BranchTimeline as BranchTimelineData,
    BranchSessionType,
  } from '../../types';
  import * as commands from '../../commands';
  import type { ProjectAction } from '../../commands';
  import BranchTimeline from '../timeline/BranchTimeline.svelte';
  import DiffModal from '../diff/DiffModal.svelte';
  import SessionModal from '../sessions/SessionModal.svelte';
  import NewSessionModal from '../sessions/NewSessionModal.svelte';
  import NoteModal from '../notes/NoteModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import ActionOutputModal from '../actions/ActionOutputModal.svelte';
  import { runBranchAction, type ActionStatusEvent } from '../actions/actions';
  import { getAvailableOpeners, openInApp, copyPathToClipboard, type OpenerApp } from './branch';
  import { getPreferredAgent } from '../settings/preferences.svelte';
  import { agentState, REMOTE_AGENTS } from '../agents/agent.svelte';

  interface Props {
    branch: Branch;
    deleting?: boolean;
    onDelete?: () => void;
  }

  let { branch, deleting = false, onDelete }: Props = $props();

  // =========================================================================
  // PR button state
  // =========================================================================
  type PrState = 'idle' | 'creating' | 'error' | 'created';
  let prStateOverride = $state<PrState | null>(null);
  let prState = $derived<PrState>(prStateOverride ?? (branch.prNumber ? 'created' : 'idle'));
  let prSessionId = $state<string | null>(null);
  let prError = $state<string | null>(null);
  let prUrl = $state<string | null>(null);
  let showPrErrorDialog = $state(false);

  // Unpushed-commits state (only relevant when PR already exists)
  let hasUnpushed = $state(false);
  let pushing = $state(false);
  let pushError = $state<string | null>(null);
  let showPushErrorDialog = $state(false);
  let forcePushing = $state(false);

  // Dropdown state
  let showMoreMenu = $state(false);
  let showActionsSubmenu = $state(false);
  let actionsSubmenuTimeout = $state<ReturnType<typeof setTimeout> | null>(null);
  let showOpenInSubmenu = $state(false);
  let openInSubmenuTimeout = $state<ReturnType<typeof setTimeout> | null>(null);
  let openerApps = $state<OpenerApp[]>([]);

  let timeline = $state<BranchTimelineData | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let showBranchDiff = $state(false);

  // Whether the branch has any commits (used to gate PR button visibility)
  let hasCommits = $derived(timeline != null && timeline.commits.length > 0);

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
  // Listen for project actions changes to refresh actions list
  function handleActionsChanged(event: CustomEvent) {
    if (event.detail?.projectId === branch.projectId) {
      loadActions();
    }
  }

  $effect(() => {
    const branchId = branch.id;
    const branchName = branch.branchName;
    console.log(
      '[BranchCard] Setting up listeners for branch:',
      () => branchId,
      () => branchName
    );

    listen<{
      sessionId: string;
      status: string;
    }>('session-status-changed', (event) => {
      const { sessionId: eventSessionId, status } = event.payload;
      if (status === 'completed' || status === 'error' || status === 'cancelled') {
        loadTimeline();
        // Handle PR session completion
        if (eventSessionId === prSessionId) {
          handlePrSessionComplete(status);
        }
      }
    }).then((unlisten) => {
      unlistenStatus = unlisten;
      console.log('[BranchCard] Session status listener registered for:', () => branchId);
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
          () => branchId
        );
        return;
      }

      console.log(
        '[BranchCard] Processing action_status for branch:',
        () => branchId,
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
      console.log('[BranchCard] Action status listener registered for:', () => branchId);
    });

    return () => {
      unlistenStatus?.();
      unlistenActionStatus?.();
    };
  });

  // Fallback polling: if the session-status-changed event is missed (e.g. due to
  // listener re-registration race), poll the session status every 5s while creating.
  $effect(() => {
    if (prState !== 'creating' || !prSessionId) return;

    const sid = prSessionId;
    const interval = setInterval(async () => {
      try {
        const session = await commands.getSession(sid);
        if (session && session.status !== 'running') {
          handlePrSessionComplete(session.status);
        }
      } catch {
        // Session may have been deleted — clear the creating state
        prStateOverride = 'error';
        prError = 'Lost track of PR creation session.';
        prSessionId = null;
      }
    }, 5_000);

    return () => clearInterval(interval);
  });

  // Re-check unpushed commits whenever the timeline refreshes and a PR exists
  $effect(() => {
    // Re-run when timeline changes (dependency) and PR exists
    if (timeline && branch.prNumber && branch.branchType === 'local') {
      commands.hasUnpushedCommits(branch.id).then((v) => (hasUnpushed = v));
    }
  });

  onMount(() => {
    loadTimeline();
    loadActions();
    getAvailableOpeners().then((apps) => (openerApps = apps));
    // Listen for actions changes
    window.addEventListener('project-actions-changed', handleActionsChanged as EventListener);
  });

  onDestroy(() => {
    unlistenStatus?.();
    unlistenActionStatus?.();
    // Clean up actions listener
    window.removeEventListener('project-actions-changed', handleActionsChanged as EventListener);
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

  // Open In submenu handlers
  function handleOpenInSubmenuEnter() {
    if (openInSubmenuTimeout) {
      clearTimeout(openInSubmenuTimeout);
      openInSubmenuTimeout = null;
    }
    showOpenInSubmenu = true;
  }

  function handleOpenInSubmenuLeave() {
    openInSubmenuTimeout = setTimeout(() => {
      showOpenInSubmenu = false;
      openInSubmenuTimeout = null;
    }, 100);
  }

  async function handleOpenInApp(appId: string) {
    showMoreMenu = false;
    showOpenInSubmenu = false;
    if (branch.worktreePath) {
      await openInApp(branch.worktreePath, appId);
    }
  }

  async function handleCopyPath() {
    showMoreMenu = false;
    showOpenInSubmenu = false;
    if (branch.worktreePath) {
      await copyPathToClipboard(branch.worktreePath);
    }
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

  // =========================================================================
  // PR creation
  // =========================================================================

  /**
   * Extract a PR URL from session messages.
   *
   * Searches in two passes:
   *  1. Look for the explicit `PR_URL: <url>` marker the agent is instructed
   *     to emit — checked in assistant AND tool_result messages because the
   *     marker may appear in shell output captured as a tool_result.
   *  2. Fall back to any GitHub PR URL (`/pull/\d+`) found in any message
   *     role, which covers `gh pr create` output stored as a tool_result.
   */
  function extractPrUrl(messages: { content: string; role: string }[]): string | null {
    // Pass 1: explicit PR_URL marker (highest confidence)
    for (const msg of messages) {
      if (msg.role !== 'assistant' && msg.role !== 'tool_result') continue;
      const markerMatch = msg.content.match(/PR_URL:\s*(https?:\/\/\S+)/);
      if (markerMatch) return markerMatch[1];
    }
    // Pass 2: any GitHub PR URL in any message
    for (const msg of messages) {
      const ghMatch = msg.content.match(/https:\/\/github\.com\/[^/]+\/[^/]+\/pull\/\d+/);
      if (ghMatch) return ghMatch[0];
    }
    return null;
  }

  /**
   * Extract the PR number from a GitHub PR URL.
   */
  function extractPrNumber(url: string): number | null {
    const match = url.match(/\/pull\/(\d+)/);
    return match ? parseInt(match[1], 10) : null;
  }

  /**
   * Build the PR URL from the branch's PR number.
   * We store only the number, so we need the repo URL to reconstruct.
   * Falls back to null if we can't determine the repo URL.
   */
  function getPrUrlFromNumber(prNumber: number): string | null {
    // If we captured the URL during creation, use it
    if (prUrl) return prUrl;
    // Otherwise we can't reconstruct without the repo URL — return null
    // The user will need to view from the repo directly
    return null;
  }

  async function handleCreatePr() {
    if (prState === 'creating') return;

    prStateOverride = 'creating';
    prError = null;
    prUrl = null;

    try {
      // Pick the best available agent for this branch's location (local vs remote)
      const remote = branch.branchType === 'remote';
      const agents = remote ? REMOTE_AGENTS : agentState.providers;
      const provider = getPreferredAgent(agents) ?? undefined;
      const sessionId = await commands.createPr(branch.id, provider);
      prSessionId = sessionId;
      // The session-status-changed listener will handle completion
    } catch (e) {
      prStateOverride = 'error';
      prError = e instanceof Error ? e.message : String(e);
    }
  }

  async function handlePrSessionComplete(status: string) {
    if (status === 'completed' && prSessionId) {
      try {
        // Fetch session messages to find the PR URL
        const messages = await commands.getSessionMessages(prSessionId);
        const foundUrl = extractPrUrl(messages);

        if (foundUrl) {
          prUrl = foundUrl;
          const prNumber = extractPrNumber(foundUrl);
          if (prNumber) {
            // Save PR number to storage
            await commands.updateBranchPr(branch.id, prNumber);
            branch.prNumber = prNumber;
          }
          prStateOverride = 'created';
        } else {
          // Session completed but we couldn't find a PR URL
          prStateOverride = 'error';
          prError = 'PR session completed but no PR URL was found in the output.';
        }
      } catch (e) {
        prStateOverride = 'error';
        prError = e instanceof Error ? e.message : String(e);
      }
    } else {
      // Session errored or was cancelled
      prStateOverride = 'error';
      prError = `PR creation session ${status === 'error' ? 'failed' : 'was cancelled'}.`;
    }
    prSessionId = null;
  }

  async function handlePush(force = false) {
    pushing = true;
    pushError = null;
    try {
      await commands.pushBranch(branch.id, force);
      hasUnpushed = false;
    } catch (e) {
      console.error('Push failed:', e);
      pushError = e instanceof Error ? e.message : String(e);
    } finally {
      pushing = false;
    }
  }

  async function handleForcePush() {
    forcePushing = true;
    try {
      await commands.pushBranch(branch.id, true);
      hasUnpushed = false;
      pushError = null;
      showPushErrorDialog = false;
    } catch (e) {
      console.error('Force push failed:', e);
      pushError = e instanceof Error ? e.message : String(e);
    } finally {
      forcePushing = false;
    }
  }

  function handlePushErrorClose() {
    showPushErrorDialog = false;
    pushError = null;
  }

  function handlePrButtonClick() {
    if (pushError) {
      // Push failed — show error dialog
      showPushErrorDialog = true;
      return;
    }
    if (prState === 'created' && hasUnpushed) {
      handlePush();
    } else if (prState === 'created') {
      // View PR - open in browser
      const url = prUrl || (branch.prNumber ? getPrUrlFromNumber(branch.prNumber) : null);
      if (url) {
        commands.openUrl(url);
      }
    } else if (prState === 'error') {
      // Show error dialog
      showPrErrorDialog = true;
    } else if (prState === 'idle') {
      handleCreatePr();
    } else if (prState === 'creating' && prSessionId) {
      // While creating, open the session chat so user can watch progress
      openSessionId = prSessionId;
    }
  }

  function handlePrErrorRetry() {
    showPrErrorDialog = false;
    handleCreatePr();
  }

  function handlePrErrorClose() {
    showPrErrorDialog = false;
    prStateOverride = null;
    prError = null;
  }
</script>

<svelte:window onclick={handleClickOutside} />

<div
  class="branch-card"
  class:deleting
  class:creating-worktree={branch.branchType === 'local' && !branch.worktreePath && !deleting}
>
  {#if deleting}
    <div class="deleting-overlay">
      <Spinner size={16} />
      <span>Deleting…</span>
    </div>
  {:else if branch.branchType === 'local' && !branch.worktreePath}
    <div class="card-header">
      <div class="branch-info">
        <GitBranch size={16} class="branch-icon" />
        <span class="branch-name">{branch.branchName}</span>
        <span class="branch-separator">›</span>
        <span class="base-branch-name">{formatBaseBranch(branch.baseBranch)}</span>
      </div>
    </div>
    <div class="card-content">
      <div class="loading">
        <Spinner size={14} />
        <span>Creating worktree…</span>
      </div>
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
                <Spinner size={12} />
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
                <Spinner size={13} />
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

              <!-- Open In submenu -->
              {#if branch.worktreePath && openerApps.length > 0}
                <div class="menu-separator"></div>
                <div class="submenu-container">
                  <button
                    class="more-menu-item submenu-trigger"
                    onmouseenter={handleOpenInSubmenuEnter}
                    onmouseleave={handleOpenInSubmenuLeave}
                  >
                    <ExternalLink size={14} />
                    Open In
                    <ChevronDown size={12} class="submenu-chevron" />
                  </button>
                  {#if showOpenInSubmenu}
                    <div
                      class="submenu"
                      role="group"
                      onmouseenter={handleOpenInSubmenuEnter}
                      onmouseleave={handleOpenInSubmenuLeave}
                    >
                      {#each openerApps as app (app.id)}
                        <button class="more-menu-item" onclick={() => handleOpenInApp(app.id)}>
                          {app.name}
                        </button>
                      {/each}
                      <div class="menu-separator"></div>
                      <button class="more-menu-item" onclick={handleCopyPath}>
                        <Copy size={14} />
                        Copy Path
                      </button>
                    </div>
                  {/if}
                </div>
              {:else if branch.worktreePath}
                <div class="menu-separator"></div>
                <button class="more-menu-item" onclick={handleCopyPath}>
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
          onSessionClick={handleTimelineSessionClick}
          onCommitClick={handleCommitClick}
          onNoteClick={handleNoteClick}
          onDeleteCommit={handleDeleteCommit}
          onDeletePendingCommit={handleDeletePendingCommit}
          onDeleteNote={handleDeleteNote}
        />
      {/if}
    </div>

    <!-- Footer with PR button and note/commit buttons -->
    <div class="card-footer">
      {#if hasCommits || prState !== 'idle'}
        <button
          class="pr-btn"
          class:creating={prState === 'creating'}
          class:error={(prState === 'error' || !!pushError) && !showPushErrorDialog}
          class:created={prState === 'created' && !pushError}
          class:pushing
          onclick={handlePrButtonClick}
          disabled={pushing || showPushErrorDialog}
          title={pushing
            ? 'Pushing…'
            : pushError
              ? 'Push failed — click for details'
              : prState === 'created' && hasUnpushed
                ? 'Push new commits'
                : prState === 'created'
                  ? 'View PR'
                  : prState === 'error'
                    ? 'PR creation failed — click for details'
                    : prState === 'creating'
                      ? 'Creating PR… (click to view)'
                      : 'Create PR'}
        >
          {#if pushing}
            <Spinner size={13} />
          {:else if pushError}
            <AlertCircle size={13} />
          {:else if prState === 'creating'}
            <Spinner size={13} />
          {:else if prState === 'error'}
            <AlertCircle size={13} />
          {:else if prState === 'created' && hasUnpushed}
            <GitPullRequestCreateArrow size={13} />
          {:else if prState === 'created'}
            <GitPullRequestArrow size={13} />
          {:else}
            <GitPullRequestCreateArrow size={13} />
          {/if}
          <span>
            {#if pushing}
              Pushing…
            {:else if pushError}
              Push failed
            {:else if prState === 'created' && hasUnpushed}
              Push{#if branch.prNumber}&nbsp;#{branch.prNumber}{/if}
            {:else if prState === 'created'}
              View PR{#if branch.prNumber}&nbsp;#{branch.prNumber}{/if}
            {:else if prState === 'creating'}
              Creating PR…
            {:else if prState === 'error'}
              PR failed
            {:else}
              Create PR
            {/if}
          </span>
        </button>
      {/if}
      <div class="new-btn-group">
        <button
          class="new-item-btn note-btn"
          onclick={() => openNewSession('note')}
          disabled={showNewSession}
          title="New note"
        >
          <FileText size={13} />
          <span>New note</span>
        </button>
        <button
          class="new-item-btn commit-btn"
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
    onClose={async () => {
      const closedSessionId = openSessionId;
      openSessionId = null;
      loadTimeline();
      // If the closed modal was the PR session, check if it finished while open
      if (prState === 'creating' && closedSessionId === prSessionId && closedSessionId) {
        try {
          const session = await commands.getSession(closedSessionId);
          if (session && session.status !== 'running') {
            handlePrSessionComplete(session.status);
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

{#if actionOutputModal}
  <ActionOutputModal
    executionId={actionOutputModal.executionId}
    actionName={actionOutputModal.actionName}
    onClose={() => (actionOutputModal = null)}
  />
{/if}

{#if showPrErrorDialog}
  <ConfirmDialog
    title="PR Creation Failed"
    message={prError ?? 'An unknown error occurred while creating the PR.'}
    confirmLabel="Retry"
    onConfirm={handlePrErrorRetry}
    onCancel={handlePrErrorClose}
  />
{/if}

{#if showPushErrorDialog}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="modal-backdrop"
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    onclick={(e) => {
      if (e.target === e.currentTarget && !forcePushing) handlePushErrorClose();
    }}
    onkeydown={(e) => {
      if (e.key === 'Escape' && !forcePushing) handlePushErrorClose();
    }}
  >
    <div class="push-error-modal">
      <div class="push-error-content">
        <div class="push-error-icon-wrapper">
          <AlertCircle size={24} />
        </div>
        <div class="push-error-text">
          <h2>Push Failed</h2>
          <p>{pushError ?? 'An unknown error occurred while pushing.'}</p>
        </div>
      </div>
      <div class="push-error-actions">
        <button class="btn btn-secondary" onclick={handlePushErrorClose} disabled={forcePushing}>
          Close
        </button>
        <button class="btn btn-danger" onclick={handleForcePush} disabled={forcePushing}>
          {#if forcePushing}
            <Spinner size={13} />
          {/if}
          Force Push
        </button>
      </div>
    </div>
  </div>
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
    color: var(--branch-color);
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
    justify-content: space-between;
    align-items: center;
    padding: 6px 12px;
  }

  /* PR button */
  .pr-btn {
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

  .pr-btn:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--border-muted);
    background: var(--bg-hover);
  }

  .pr-btn:disabled {
    cursor: default;
  }

  .pr-btn.creating {
    color: var(--text-muted);
    border-color: var(--border-muted);
  }

  .pr-btn.error {
    color: var(--ui-danger);
    border-color: var(--ui-danger);
  }

  .pr-btn.error:hover {
    background: var(--ui-danger-bg);
  }

  .pr-btn.created {
    color: var(--text-faint);
    border-color: var(--border-subtle);
  }

  .pr-btn.created:hover {
    color: var(--text-primary);
    border-color: var(--border-muted);
    background: var(--bg-hover);
  }

  .pr-btn.pushing {
    color: var(--text-muted);
    border-color: var(--border-muted);
    cursor: default;
  }

  .pr-btn :global(svg) {
    flex-shrink: 0;
  }

  /* Footer with separate note/commit buttons */
  .new-btn-group {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: auto;
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
    transition: color 0.15s;
  }

  /* Icon color on button hover */
  .note-btn:hover :global(svg) {
    color: var(--note-color);
  }

  .commit-btn:hover :global(svg) {
    color: var(--commit-color);
  }

  :global(.spinner) {
    animation: spin 1s linear infinite;
    flex-shrink: 0;
  }

  /* Push error dialog */
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .push-error-modal {
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 400px;
    max-width: 90vw;
    overflow: hidden;
  }

  .push-error-content {
    display: flex;
    gap: 16px;
    padding: 24px;
  }

  .push-error-icon-wrapper {
    flex-shrink: 0;
    width: 40px;
    height: 40px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--ui-danger-bg);
    border-radius: 10px;
    color: var(--ui-danger);
  }

  .push-error-text {
    flex: 1;
    min-width: 0;
  }

  .push-error-text h2 {
    margin: 0 0 8px 0;
    font-size: var(--size-base);
    font-weight: 600;
    color: var(--text-primary);
  }

  .push-error-text p {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
    line-height: 1.5;
    word-break: break-word;
  }

  .push-error-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 16px 24px;
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-primary);
  }

  .btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border: none;
    border-radius: 6px;
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition:
      background-color 0.1s,
      opacity 0.1s;
  }

  .btn:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .btn-secondary {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-secondary:hover:not(:disabled) {
    background: var(--border-subtle);
  }

  .btn-danger {
    background: var(--ui-danger);
    color: white;
  }

  .btn-danger:hover:not(:disabled) {
    filter: brightness(1.1);
  }
</style>
