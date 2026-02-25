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
  import { untrack } from 'svelte';
  import { slide, fade } from 'svelte/transition';
  import {
    GitBranch,
    GitCommitVertical,
    GitPullRequestCreateArrow,
    GitPullRequestArrow,
    GitPullRequestDraft,
    GitMerge,
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
  import { subscribeDragDrop } from './dragDrop';
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
  import {
    runBranchAction,
    getRunningBranchActions,
    clearActionExecution,
    type ActionStatusEvent,
  } from '../actions/actions';
  import { getAvailableOpeners, openInApp, copyPathToClipboard, type OpenerApp } from './branch';
  import {
    extractPrNumber,
    extractPrUrl,
    fileNameFromPath,
    formatBaseBranch,
    getPrimaryActionExecution,
    getPrimaryRunAction,
    getActionTypeLabel,
    getRemainingRunActions,
    getSecondaryRunningActions,
    groupActionsByType,
    isPushRejectedNonFastForward,
    isTextFile,
  } from './branchCardHelpers';
  import { getPreferredAgent } from '../settings/preferences.svelte';
  import { agentState, REMOTE_AGENTS } from '../agents/agent.svelte';
  import { prStateStore, type PrState } from '../../stores/prState.svelte';
  import { pushStateStore, type PushState } from '../../stores/pushState.svelte';
  import BranchCardHeaderInfo from './BranchCardHeaderInfo.svelte';
  import ReasonBanner from './ReasonBanner.svelte';
  import { alerts } from '../../shared/alerts.svelte';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import { sessionRegistry } from '../../stores/sessionRegistry.svelte';

  interface Props {
    branch: Branch;
    repoLabel?: { githubRepo: string; subpath: string | null; reason?: string | null } | null;
    projectName?: string;
    deleting?: boolean;
    worktreeError?: string;
    onDelete?: () => void;
    onRename?: (branchName: string) => void;
    onRetryWorktree?: () => void;
  }

  let {
    branch,
    repoLabel = null,
    projectName,
    deleting = false,
    worktreeError,
    onDelete,
    onRename,
    onRetryWorktree,
  }: Props = $props();

  function notifyError(title: string, e: unknown): void {
    alerts.show({
      tone: 'error',
      title,
      message: e instanceof Error ? e.message : String(e),
      durationMs: 0,
    });
  }

  // =========================================================================
  // Option-key tracking (for draft PR creation)
  // =========================================================================
  let optionHeld = $state(false);

  function handleOptionDown(e: KeyboardEvent) {
    if (e.key === 'Alt') optionHeld = true;
  }
  function handleOptionUp(e: KeyboardEvent) {
    if (e.key === 'Alt') optionHeld = false;
  }

  // =========================================================================
  // PR button state — derived directly from the global prStateStore
  //
  // This ensures the spinner/state is always in sync with the store,
  // even if the component was unmounted while App.svelte's global
  // session-status-changed handler updated the store.
  // =========================================================================
  let storePrState = $derived(prStateStore.getPrState(branch.id));
  let prState = $derived<PrState>(
    // If PR already exists (has prNumber) but store still says 'creating',
    // treat it as 'created' (stale store entry)
    storePrState
      ? branch.prNumber && storePrState.state === 'creating'
        ? 'created'
        : storePrState.state
      : branch.prNumber
        ? 'created'
        : 'idle'
  );
  let prSessionId = $derived(storePrState?.sessionId ?? null);
  let prError = $derived(storePrState?.error ?? null);
  let prUrl = $derived(storePrState?.url ?? null);
  // Local cache for PR URLs fetched lazily via getPrUrl (not stored in prStateStore)
  let cachedPrUrl = $state<string | null>(null);
  let showPrErrorDialog = $state(false);

  // Clean up stale 'creating' entries when branch already has a PR number
  $effect(() => {
    const stored = prStateStore.getPrState(branch.id);
    if (stored && branch.prNumber && stored.state === 'creating') {
      prStateStore.clearPrState(branch.id);
    }
  });

  // Unpushed-commits state (only relevant when PR already exists)
  let hasUnpushed = $state(false);

  // PR status polling state
  let prStatusPollTimer: ReturnType<typeof setInterval> | null = null;
  let prStatusRefreshing = $state(false);

  // PR status fields (local state, updated via events)
  // Initialize to null, $effect will sync with branch prop
  let prStatusState = $state<string | null>(null);
  let prStatusChecks = $state<string | null>(null);
  let prStatusReviewDecision = $state<string | null>(null);
  let prStatusMergeable = $state<boolean | null>(null);
  let prStatusDraft = $state<boolean | null>(null);

  // Sync local PR status state when branch prop changes
  $effect(() => {
    prStatusState = branch.prState;
    prStatusChecks = branch.prChecksStatus;
    prStatusReviewDecision = branch.prReviewDecision;
    prStatusMergeable = branch.prMergeable;
    prStatusDraft = branch.prDraft;
  });

  // =========================================================================
  // Push button state — derived directly from the global pushStateStore
  //
  // This ensures the spinner/state is always in sync with the store,
  // even if the component was unmounted while App.svelte's global
  // session-status-changed handler updated the store.
  // =========================================================================
  let storePushState = $derived(pushStateStore.getPushState(branch.id));
  let pushState = $derived<PushState>(storePushState?.state ?? 'idle');
  let pushSessionId = $derived(storePushState?.sessionId ?? null);
  let pushError = $derived(storePushState?.error ?? null);
  let pushRejectedNonFastForward = $derived(storePushState?.rejectedNonFastForward ?? false);
  let showPushErrorDialog = $state(false);
  let showForcePushDialog = $state(false);

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
  let loadedTimelineKey = $state<string | null>(null);

  /** True when the branch has at least one finalized commit (code changes vs base). */
  let hasCodeChanges = $derived(timeline?.commits.some((c) => !!c.sha) ?? false);

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
  let unlistenPrStatus: UnlistenFn | null = null;

  // Window focus handlers (stored for cleanup)
  let handleFocus: (() => void) | null = null;
  let handleBlur: (() => void) | null = null;

  // Set up event listeners immediately (synchronously) at module level like old codebase
  // Listen for project actions changes to refresh actions list
  function handleActionsChanged(event: CustomEvent) {
    if (!event.detail?.projectId || event.detail?.projectId === branch.projectId) {
      loadActions();
    }
  }

  $effect(() => {
    const branchId = branch.id;

    listen<{
      sessionId: string;
      status: string;
      branchId?: string;
    }>('session-status-changed', (event) => {
      const { sessionId: eventSessionId, status, branchId: eventBranchId } = event.payload;
      if (status === 'completed' || status === 'error' || status === 'cancelled') {
        loadTimeline();
        // Handle PR session completion
        if (eventSessionId === prSessionId) {
          handlePrSessionComplete(status);
        }
        // Handle push session completion
        if (eventSessionId === pushSessionId) {
          handlePushSessionComplete(status);
        }
      } else if (status === 'running' && eventBranchId === branchId) {
        // An MCP-initiated session just started in this branch — refresh the
        // timeline so the pending note/commit stub appears immediately.
        loadTimeline();
      }
    }).then((unlisten) => {
      unlistenStatus = unlisten;
    });

    listen<ActionStatusEvent>('action_status', (event) => {
      const payload = event.payload;

      // Only process events for this branch
      if (payload.branchId !== branchId) {
        return;
      }

      const existingIndex = runningActions.findIndex((a) => a.executionId === payload.executionId);

      if (payload.status === 'running') {
        if (existingIndex === -1) {
          runningActions.push({
            executionId: payload.executionId,
            actionId: payload.actionId,
            actionName: payload.actionName,
            status: 'running',
            startedAt: payload.startedAt ?? Date.now(),
          });
        }
      } else {
        // Action completed/failed/stopped - update status
        if (existingIndex !== -1) {
          runningActions[existingIndex].status = payload.status as any;
          runningActions[existingIndex].exitCode = payload.exitCode;
          runningActions[existingIndex].completedAt = payload.completedAt;

          // Auto-remove terminal states after a delay
          const action = runningActions[existingIndex];
          const isPrimaryAction = primaryRunAction && action.actionId === primaryRunAction.id;

          // Determine delay based on status: completed shows briefly, stopped/failed show longer
          let displayTime: number;
          if (payload.status === 'completed') {
            displayTime = isPrimaryAction ? 1000 : 2000;
          } else {
            // stopped/failed: show status briefly then clean up so rerun works cleanly
            displayTime = isPrimaryAction ? 2000 : 3000;
          }

          setTimeout(() => {
            const foundAction = runningActions.find((a) => a.executionId === payload.executionId);
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
          }, displayTime);
        }
      }
    }).then((unlisten) => {
      unlistenActionStatus = unlisten;
    });

    listen<{
      branchId: string;
      prState: string;
      prChecksStatus: string;
      prReviewDecision: string | null;
      prMergeable: boolean;
      prDraft: boolean;
    }>('pr-status-changed', (event) => {
      const payload = event.payload;
      if (payload.branchId === branchId) {
        // Update local PR status state
        prStatusState = payload.prState;
        prStatusChecks = payload.prChecksStatus;
        prStatusReviewDecision = payload.prReviewDecision;
        prStatusMergeable = payload.prMergeable;
        prStatusDraft = payload.prDraft;
      }
    }).then((unlisten) => {
      unlistenPrStatus = unlisten;
    });

    return () => {
      unlistenStatus?.();
      unlistenActionStatus?.();
      unlistenPrStatus?.();
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
        prStateStore.setPrError(branch.id, 'Lost track of PR creation session.');
        prStateStore.clearSessionTracking(branch.id);
      }
    }, 5_000);

    return () => clearInterval(interval);
  });

  // Fallback polling for push session (same pattern as PR)
  $effect(() => {
    if (pushState !== 'pushing' || !pushSessionId) return;

    const sid = pushSessionId;
    const interval = setInterval(async () => {
      try {
        const session = await commands.getSession(sid);
        if (session && session.status !== 'running') {
          handlePushSessionComplete(session.status);
        }
      } catch {
        // Session may have been deleted — clear the pushing state
        pushStateStore.setPushError(branch.id, 'Lost track of push session.');
        pushStateStore.clearSessionTracking(branch.id);
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

  // Load timeline when a branch becomes timeline-ready, including when a local
  // branch transitions from "creating worktree" to an attached worktree path.
  $effect(() => {
    if (branch.branchType === 'local' && !branch.worktreePath) return;

    const timelineKey =
      branch.branchType === 'remote'
        ? `${branch.id}:<remote>`
        : `${branch.id}:${branch.worktreePath}`;
    if (timelineKey === loadedTimelineKey) return;

    loadedTimelineKey = timelineKey;
    loadTimeline();
  });

  // Track window focus for smart polling
  let isWindowFocused = $state(true);

  // PR status polling: adaptive intervals based on status
  $effect(() => {
    // Determine if we should poll and at what interval
    const shouldPoll = branch.prNumber && isWindowFocused;

    // Don't poll if PR is merged or closed
    if (prStatusState === 'MERGED' || prStatusState === 'CLOSED') {
      if (prStatusPollTimer) {
        clearInterval(prStatusPollTimer);
        prStatusPollTimer = null;
      }
      return;
    }

    // Choose interval based on status
    let pollInterval: number;
    if (prStatusChecks === 'PENDING') {
      // Checks are running - poll frequently
      pollInterval = 15_000; // 15 seconds
    } else {
      // Checks passed/failed or no status - poll less frequently
      pollInterval = 60_000; // 60 seconds
    }

    if (shouldPoll) {
      // Restart polling if interval changed
      if (prStatusPollTimer) {
        clearInterval(prStatusPollTimer);
      }

      prStatusPollTimer = setInterval(async () => {
        if (prStatusRefreshing) return; // Skip if already refreshing
        try {
          prStatusRefreshing = true;
          await commands.refreshPrStatus(branch.id);
          // Status will be updated via pr-status-changed event
        } catch (e) {
          console.error('Failed to refresh PR status:', e);
        } finally {
          prStatusRefreshing = false;
        }
      }, pollInterval);
    } else {
      // Stop polling when window not focused
      if (prStatusPollTimer) {
        clearInterval(prStatusPollTimer);
        prStatusPollTimer = null;
      }
    }

    return () => {
      if (prStatusPollTimer) {
        clearInterval(prStatusPollTimer);
        prStatusPollTimer = null;
      }
    };
  });

  onMount(() => {
    loadActions();
    loadRunningActions();
    getAvailableOpeners().then((apps) => (openerApps = apps));
    // Listen for actions changes
    window.addEventListener('project-actions-changed', handleActionsChanged as EventListener);

    // Option-key tracking for draft PR creation
    window.addEventListener('keydown', handleOptionDown);
    window.addEventListener('keyup', handleOptionUp);

    // Window focus tracking for smart polling
    handleFocus = () => {
      isWindowFocused = true;
      // Immediately refresh PR status when app becomes active
      if (branch.prNumber && !prStatusRefreshing) {
        commands
          .refreshPrStatus(branch.id)
          .catch((e) => console.error('Failed to refresh PR status on focus:', e));
      }
    };
    handleBlur = () => {
      isWindowFocused = false;
    };
    window.addEventListener('focus', handleFocus);
    window.addEventListener('blur', handleBlur);

    // Fetch initial PR status if PR exists
    if (branch.prNumber) {
      commands
        .refreshPrStatus(branch.id)
        .catch((e) => console.error('Failed to fetch initial PR status:', e));
      // Status will be updated via pr-status-changed event
    }
  });

  onDestroy(() => {
    unlistenStatus?.();
    unlistenActionStatus?.();
    unlistenPrStatus?.();
    // Clean up PR status polling
    if (prStatusPollTimer) {
      clearInterval(prStatusPollTimer);
      prStatusPollTimer = null;
    }
    // Clean up actions listener
    window.removeEventListener('project-actions-changed', handleActionsChanged as EventListener);
    // Clean up window focus listeners
    if (handleFocus) window.removeEventListener('focus', handleFocus);
    if (handleBlur) window.removeEventListener('blur', handleBlur);
    // Clean up option-key listeners
    window.removeEventListener('keydown', handleOptionDown);
    window.removeEventListener('keyup', handleOptionUp);
  });
  async function loadTimeline() {
    // Only show the loading spinner on the initial load. Subsequent refreshes
    // keep the existing timeline visible to avoid a jarring flash/re-render
    // that was causing UI freezes after drag-and-drop note creation.
    const isInitialLoad = !timeline;
    if (isInitialLoad) {
      loading = true;
    }
    error = null;
    try {
      timeline = await commands.getBranchTimeline(branch.id, { force: !isInitialLoad });
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function loadActions() {
    try {
      // Load actions for this branch's project
      actions = await commands.listProjectActions(branch.projectId, branch.projectRepoId);
    } catch (e) {
      console.error('Failed to load actions:', e);
      actions = [];
    }
  }

  async function loadRunningActions() {
    try {
      // Restore running actions that were started before component mounted
      const running = await getRunningBranchActions(branch.id);

      // Add each running action to state
      for (const info of running) {
        const existingIndex = runningActions.findIndex((a) => a.executionId === info.executionId);
        if (existingIndex === -1) {
          runningActions.push({
            executionId: info.executionId,
            actionId: info.actionId,
            actionName: info.actionName,
            status: 'running',
            startedAt: info.startedAt,
          });
        }
      }
    } catch (e) {
      console.error('Failed to load running actions:', e);
    }
  }

  // Group actions by type
  let groupedActions = $derived.by(() => {
    return groupActionsByType(actions);
  });

  // Get the primary run action (first run action)
  let primaryRunAction = $derived.by(() => {
    return getPrimaryRunAction(groupedActions);
  });

  // Get remaining run actions (excluding the primary one)
  let remainingRunActions = $derived.by(() => {
    return getRemainingRunActions(groupedActions);
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
    return getPrimaryActionExecution(runningActions, primaryRunAction?.id ?? null);
  });

  // Filter running actions to exclude the primary action
  let secondaryRunningActions = $derived.by(() => {
    return getSecondaryRunningActions(runningActions, primaryRunAction?.id ?? null);
  });

  async function handleRunAction(action: ProjectAction) {
    showMoreMenu = false;

    // Check if this action is currently running
    const existingExecution = runningActions.find(
      (a) => a.actionId === action.id && a.status === 'running'
    );

    if (existingExecution) {
      // Action is actively running, open modal to view output
      actionOutputModal = {
        executionId: existingExecution.executionId,
        actionName: action.name,
      };
      return;
    }

    // Remove any stale (stopped/failed/completed) entries for this action
    // before starting a new run, and clean up their backend buffers
    const staleExecutions = runningActions.filter(
      (a) => a.actionId === action.id && a.status !== 'running'
    );
    for (const stale of staleExecutions) {
      clearActionExecution(stale.executionId).catch(() => {});
    }
    runningActions = runningActions.filter(
      (a) => !(a.actionId === action.id && a.status !== 'running')
    );

    // Start the action silently (don't open modal)
    try {
      await runBranchAction(branch.id, action.id);
      // The running action will be added via the event listener
      // Don't auto-show output modal - user can click to view
    } catch (e) {
      console.error('Failed to run action:', e);
      notifyError(`Failed to run action "${action.name}"`, e);
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

  function handleRenameFromMenu() {
    showMoreMenu = false;
    const next = window.prompt('Rename branch', branch.branchName);
    if (!next) return;
    const trimmed = next.trim();
    if (!trimmed || trimmed === branch.branchName) return;
    onRename?.(trimmed);
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

  // =========================================================================
  // PR status display
  // =========================================================================

  /** Get concise PR status text for the button */
  function getPrStatusText(): string | null {
    if (!branch.prNumber) return null;

    // Check PR state first
    if (prStatusState === 'MERGED') return 'Merged';
    if (prStatusState === 'CLOSED') return 'Closed';
    if (prStatusDraft) return 'Draft';

    // Check checks status
    if (prStatusChecks === 'FAILURE') return 'Checks failing';
    if (prStatusChecks === 'PENDING') return 'Checks pending';

    // Check review decision
    if (prStatusReviewDecision === 'CHANGES_REQUESTED') return 'Changes requested';
    if (prStatusReviewDecision === 'APPROVED' && prStatusMergeable) return 'Approved';
    if (prStatusReviewDecision === 'APPROVED') return 'Approved';

    // Check mergeable status
    if (prStatusMergeable === false) return 'Has conflicts';
    if (prStatusChecks === 'SUCCESS') return 'Open';

    return null; // No specific status to show
  }

  let prStatusText = $derived(getPrStatusText());

  /** Get the status indicator color for the PR button */
  function getPrStatusIndicator(): 'success' | 'warning' | 'error' | 'neutral' | 'pending' | null {
    // Push/PR creation states - no indicator during creation (spinner is enough)
    if (prState === 'creating') return null;
    if (pushState === 'pushing') return 'pending';
    if (pushState === 'error' || prState === 'error') return 'error';

    if (!branch.prNumber) return null;

    // No indicator when showing "Push changes" button (PR exists but has unpushed commits)
    if (prState === 'created' && hasUnpushed && pushState === 'idle') return null;

    // PR exists - check status
    if (prStatusState === 'MERGED') return null; // No indicator for merged PRs
    if (prStatusState === 'CLOSED') return 'neutral';
    if (prStatusDraft) return 'neutral';

    // Mergeable status — check before checks/review so conflicts always show as red
    if (prStatusMergeable === false) return 'error';

    // Check-based states
    if (prStatusChecks === 'FAILURE') return 'error';
    if (prStatusChecks === 'PENDING') return 'pending';
    if (prStatusChecks === 'SUCCESS') return 'success';

    // Review-based states
    if (prStatusReviewDecision === 'CHANGES_REQUESTED') return 'warning';
    if (prStatusReviewDecision === 'APPROVED') return 'success';

    return 'neutral';
  }

  let prStatusIndicator = $derived(getPrStatusIndicator());

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
    // Track the running session in the project state store
    if (!result || !result.sessionId) {
      notifyError('Session Error', 'Failed to start session: no session ID returned');
      return;
    }
    // Register session in the unified registry with the actual session type
    sessionRegistry.register(result.sessionId, branch.projectId, newSessionMode, branch.id);
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

  function handleCommitClick(sha: string) {
    commitDiffSha = sha;
  }

  function handleNoteClick(_noteId: string, title: string, content: string) {
    openNote = { title, content };
  }

  function handleReviewClick(reviewId: string) {
    showBranchDiff = true;
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
      notifyError('Failed to delete pending commit', e);
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
  async function handleCreatePr(draft = false) {
    if (prState === 'creating') return;

    // Set creating state in the store immediately for instant UI feedback.
    // We use a temporary placeholder session ID; it will be replaced once
    // the real session is created.
    prStateStore.setPrCreating(branch.id, '__pending__');

    try {
      // Pick the best available agent for this branch's location (local vs remote)
      const remote = branch.branchType === 'remote';
      const agents = remote ? REMOTE_AGENTS : agentState.providers;
      const provider = getPreferredAgent(agents) ?? undefined;
      const sessionId = await commands.createPr(branch.id, provider, draft);
      // Register session in the unified registry
      sessionRegistry.register(sessionId, branch.projectId, 'pr', branch.id);
      // Store the creating state globally (now with real session ID)
      prStateStore.setPrCreating(branch.id, sessionId);
      // Track the running session in the project state store
      projectStateStore.addRunningSession(branch.projectId, sessionId);
      // The session-status-changed listener will handle completion
    } catch (e) {
      prStateStore.setPrError(branch.id, e instanceof Error ? e.message : String(e));
    }
  }

  async function handlePrSessionComplete(status: string) {
    const sid = prSessionId;
    if (status === 'completed' && sid) {
      try {
        // Fetch session messages to find the PR URL
        const messages = await commands.getSessionMessages(sid);
        const foundUrl = extractPrUrl(messages);

        if (foundUrl) {
          const prNumber = extractPrNumber(foundUrl);
          if (prNumber) {
            // Save PR number to storage
            await commands.updateBranchPr(branch.id, prNumber);
            branch.prNumber = prNumber;
            // Immediately fetch PR status after creation
            commands
              .refreshPrStatus(branch.id)
              .catch((e) => console.error('Failed to fetch initial PR status:', e));
          }
          prStateStore.setPrCreated(branch.id, foundUrl);
        } else {
          // Session completed but we couldn't find a PR URL
          prStateStore.setPrError(
            branch.id,
            'PR session completed but no PR URL was found in the output.'
          );
        }
      } catch (e) {
        prStateStore.setPrError(branch.id, e instanceof Error ? e.message : String(e));
      }
    } else {
      // Session errored or was cancelled
      prStateStore.setPrError(
        branch.id,
        `PR creation session ${status === 'error' ? 'failed' : 'was cancelled'}.`
      );
    }
    prStateStore.clearSessionTracking(branch.id);
  }

  // =========================================================================
  // Push (session-based, mirrors PR creation pattern)
  // =========================================================================

  /**
   * Check whether push session messages contain the non-fast-forward marker.
   * The agent outputs `PUSH_REJECTED: NON_FAST_FORWARD` when the remote would
   * lose commits on a normal push.
   *
   * Only checks assistant and tool_result messages to avoid matching the
   * marker in the prompt instructions (user messages) which tell the agent
   * what to output on rejection.
   */
  async function handlePush(force = false) {
    if (pushState === 'pushing') return;

    // Set pushing state in the store immediately for instant UI feedback.
    // We use a temporary placeholder session ID; it will be replaced once
    // the real session is created.
    pushStateStore.setPushing(branch.id, '__pending__');

    try {
      const remote = branch.branchType === 'remote';
      const agents = remote ? REMOTE_AGENTS : agentState.providers;
      const provider = getPreferredAgent(agents) ?? undefined;
      const sessionId = await commands.pushBranch(branch.id, provider, force);
      // Register session in the unified registry
      sessionRegistry.register(sessionId, branch.projectId, 'push', branch.id);
      // Store the pushing state globally (now with real session ID)
      pushStateStore.setPushing(branch.id, sessionId);
      // Track the running session in the project state store
      projectStateStore.addRunningSession(branch.projectId, sessionId);
      // The session-status-changed listener will handle completion
    } catch (e) {
      pushStateStore.setPushError(branch.id, e instanceof Error ? e.message : String(e));
    }
  }

  async function handlePushSessionComplete(status: string) {
    const sid = pushSessionId;
    if (status === 'completed' && sid) {
      // Check session messages for the non-fast-forward rejection marker
      try {
        const messages = await commands.getSessionMessages(sid);
        if (isPushRejectedNonFastForward(messages)) {
          // The agent stopped because the remote would lose commits.
          // Go to error state — clicking the button will open the force push dialog.
          pushStateStore.setPushError(branch.id, '', true); // rejectedNonFastForward=true
          pushStateStore.clearSessionTracking(branch.id);
          return;
        }
      } catch {
        // If we can't read messages, treat as success (push likely worked)
      }

      pushStateStore.setPushDone(branch.id);
      hasUnpushed = false;
      // Reset to idle after a brief moment so the button returns to "View PR"
      setTimeout(() => {
        pushStateStore.clearPushState(branch.id);
      }, 1_500);
    } else {
      pushStateStore.setPushError(
        branch.id,
        `Push session ${status === 'error' ? 'failed' : 'was cancelled'}.`
      );
    }
    pushStateStore.clearSessionTracking(branch.id);
  }

  function handleForcePushConfirm() {
    showForcePushDialog = false;
    handlePush(true);
  }

  function handleForcePushCancel() {
    showForcePushDialog = false;
    pushStateStore.clearPushState(branch.id);
  }

  function handlePushErrorRetry() {
    showPushErrorDialog = false;
    handlePush();
  }

  function handlePushErrorClose() {
    showPushErrorDialog = false;
    pushStateStore.clearPushState(branch.id);
  }

  function handlePrButtonClick() {
    if (pushState === 'error') {
      // Push failed — open the appropriate dialog based on failure type
      if (pushRejectedNonFastForward) {
        showForcePushDialog = true;
      } else {
        showPushErrorDialog = true;
      }
      return;
    }
    if (pushState === 'pushing' && pushSessionId) {
      // While pushing, open the session chat so user can watch progress
      openSessionId = pushSessionId;
      return;
    }
    if (prState === 'created' && hasUnpushed && pushState === 'idle') {
      handlePush();
    } else if (prState === 'created') {
      // View PR - open in browser
      const url = prUrl ?? cachedPrUrl;
      if (url) {
        commands.openUrl(url);
      } else if (branch.prNumber) {
        commands
          .getPrUrl(branch.id, branch.prNumber)
          .then((fetchedUrl) => {
            cachedPrUrl = fetchedUrl;
            commands.openUrl(fetchedUrl);
          })
          .catch((e) => console.error('Failed to get PR URL:', e));
      }
    } else if (prState === 'error') {
      // Show error dialog
      showPrErrorDialog = true;
    } else if (prState === 'idle') {
      handleCreatePr(optionHeld);
    } else if (prState === 'creating' && prSessionId) {
      // While creating, open the session chat so user can watch progress
      openSessionId = prSessionId;
    }
  }

  function handlePrErrorRetry() {
    showPrErrorDialog = false;
    prStateStore.clearPrState(branch.id);
    handleCreatePr(optionHeld);
  }

  function handlePrErrorClose() {
    showPrErrorDialog = false;
    prStateStore.clearPrState(branch.id);
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

  // Tauri v2 intercepts file drops at the OS level, so standard browser
  // drag/drop events never fire for files dragged from Finder/Explorer.
  // We use a shared drag-drop service (dragDrop.ts) that registers a single
  // global Tauri onDragDropEvent listener and dispatches to the correct card
  // via hit-testing.

  let dragOver = $state(false);
  let cardElement: HTMLDivElement | undefined = $state();

  /** Pending note placeholders shown in the timeline while files are being added. */
  let pendingDropNotes = $state<{ key: string; title: string }[]>([]);

  function handleFileDrop(paths: string[]) {
    const textPaths = paths.filter(isTextFile);
    if (textPaths.length === 0) return;

    // Show placeholder items immediately
    const placeholders = textPaths.map((filePath) => ({
      key: `drop-${Date.now()}-${filePath}`,
      title: fileNameFromPath(filePath),
    }));
    pendingDropNotes = [...pendingDropNotes, ...placeholders];

    // Process each file asynchronously without blocking the UI
    Promise.all(
      textPaths.map(async (filePath, i) => {
        try {
          const content = await commands.readTextFile(filePath);
          const title = fileNameFromPath(filePath);
          await commands.createNote(branch.id, title, content);
        } catch (e) {
          console.error('Failed to create note from dropped file:', e);
        } finally {
          // Remove this placeholder
          pendingDropNotes = pendingDropNotes.filter((p) => p.key !== placeholders[i].key);
        }
      })
    ).then(() => {
      loadTimeline();
    });
  }

  // Subscribe to the shared drag-drop service. A single global Tauri listener
  // is shared across all BranchCards, eliminating the O(N) listener storm that
  // caused UI freezes during drag-over events.
  $effect(() => {
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

<svelte:window onclick={handleClickOutside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  bind:this={cardElement}
  class="branch-card"
  class:deleting
  class:creating-worktree={branch.branchType === 'local' &&
    !branch.worktreePath &&
    !worktreeError &&
    !deleting}
  data-branch-id={branch.id}
  class:drag-over={dragOver}
>
  {#if deleting}
    <div class="deleting-overlay">
      <Spinner size={16} />
      <span>Deleting…</span>
    </div>
  {:else if branch.branchType === 'local' && !branch.worktreePath}
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
      <BranchCardHeaderInfo
        branchName={branch.branchName}
        {repoLabel}
        secondaryLabel={formatBaseBranch(branch.baseBranch)}
      />
      <div class="header-actions">
        <!-- Running actions (excluding primary action) -->
        {#each secondaryRunningActions as execution (execution.executionId)}
          <div
            class="running-action-container"
            class:fading={execution.fading}
            in:slide={{ duration: 300, axis: 'x' }}
            out:slide={{ duration: 300, axis: 'x' }}
          >
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
          <div
            class="primary-action-container"
            in:slide={{ duration: 300, axis: 'x' }}
            out:slide={{ duration: 300, axis: 'x' }}
          >
            <button
              class="primary-action-button"
              class:running={primaryActionExecution?.status === 'running'}
              class:completed={primaryActionExecution?.status === 'completed'}
              class:failed={primaryActionExecution?.status === 'failed'}
              onclick={() =>
                primaryActionExecution?.status === 'running'
                  ? handleShowActionOutput(primaryActionExecution)
                  : handleRunAction(primaryRunAction)}
              title={primaryRunAction.name}
            >
              {#if primaryActionExecution?.status === 'running'}
                <Spinner size={14} />
              {:else if primaryActionExecution?.status === 'completed'}
                <CheckCircle size={14} />
              {:else if primaryActionExecution?.status === 'failed'}
                <AlertCircle size={14} />
              {:else}
                <Play size={14} />
              {/if}
            </button>
          </div>
        {/if}
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
              <button class="more-menu-item" onclick={handleRenameFromMenu}>
                <GitBranch size={14} />
                Rename Branch
              </button>
              <div class="menu-separator"></div>
              <button class="more-menu-item danger" onclick={handleDeleteFromMenu}>
                <Trash2 size={14} />
                Delete Repo
              </button>
            </div>
          {/if}
        </div>
      </div>
    </div>

    <div class="card-content">
      <ReasonBanner reason={repoLabel?.reason} onDismiss={handleDismissReason} />
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
          {pendingDropNotes}
          onSessionClick={handleTimelineSessionClick}
          onCommitClick={handleCommitClick}
          onNoteClick={handleNoteClick}
          onReviewClick={(reviewId) => handleReviewClick(reviewId)}
          onDeleteCommit={handleDeleteCommit}
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
                  class="pr-btn"
                  class:creating={prState === 'creating'}
                  class:error={prState === 'error' || pushState === 'error'}
                  class:created={prState === 'created' && pushState !== 'error'}
                  class:pushing={pushState === 'pushing'}
                  class:merged={prState === 'created' && prStatusState === 'MERGED'}
                  onclick={handlePrButtonClick}
                  disabled={showPushErrorDialog || showForcePushDialog || showPrErrorDialog}
                  title={pushState === 'pushing'
                    ? 'Pushing… (click to view)'
                    : pushState === 'error'
                      ? 'Push failed — click for details'
                      : prState === 'created' && hasUnpushed
                        ? 'Push changes to remote'
                        : prState === 'created'
                          ? 'View PR'
                          : prState === 'error'
                            ? 'PR creation failed — click for details'
                            : prState === 'creating'
                              ? 'Creating PR… (click to view)'
                              : optionHeld
                                ? 'Create draft PR (⌥ held)'
                                : 'Create PR'}
                >
                  {#if pushState === 'pushing'}
                    <Spinner size={13} />
                  {:else if pushState === 'error'}
                    <AlertCircle size={13} />
                  {:else if prState === 'creating'}
                    <Spinner size={13} />
                  {:else if prState === 'error'}
                    <AlertCircle size={13} />
                  {:else if prState === 'created' && prStatusState === 'MERGED'}
                    <GitMerge size={13} />
                  {:else if prState === 'created' && hasUnpushed}
                    <GitPullRequestDraft size={13} />
                  {:else if prState === 'created'}
                    <GitPullRequestArrow size={13} />
                  {:else}
                    <GitPullRequestCreateArrow size={13} />
                  {/if}
                  <span>
                    {#if pushState === 'pushing'}
                      Pushing…
                    {:else if pushState === 'error'}
                      Push failed
                    {:else if prState === 'created' && hasUnpushed}
                      Push changes
                    {:else if prState === 'created'}
                      {#if prStatusText}
                        {prStatusText}
                      {:else}
                        View PR{#if branch.prNumber}&nbsp;#{branch.prNumber}{/if}
                      {/if}
                    {:else if prState === 'creating'}
                      Creating PR…
                    {:else if prState === 'error'}
                      PR failed
                    {:else}
                      {optionHeld ? 'Create draft PR' : 'Create PR'}
                    {/if}
                  </span>
                  {#if prStatusIndicator}
                    <span class="pr-status-indicator {prStatusIndicator}"></span>
                  {/if}
                </button>
                <button
                  class="pr-btn diff-btn"
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
    </div>
  {/if}
</div>

{#if showBranchDiff}
  <DiffModal
    branchId={branch.id}
    scope="branch"
    beforeLabel={formatBaseBranch(branch.baseBranch)}
    afterLabel={branch.branchName}
    {projectName}
    githubRepo={repoLabel?.githubRepo}
    subpath={repoLabel?.subpath}
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

{#if showNewSession}
  <NewSessionModal
    {branch}
    mode={newSessionMode}
    initialPrompt={draftPrompt}
    remote={branch.branchType === 'remote'}
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
      // If the closed modal was the push session, check if it finished while open
      if (pushState === 'pushing' && closedSessionId === pushSessionId && closedSessionId) {
        try {
          const session = await commands.getSession(closedSessionId);
          if (session && session.status !== 'running') {
            handlePushSessionComplete(session.status);
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
  <ConfirmDialog
    title="Push Failed"
    message={pushError ?? 'An unknown error occurred while pushing.'}
    confirmLabel="Retry"
    onConfirm={handlePushErrorRetry}
    onCancel={handlePushErrorClose}
  />
{/if}

{#if showForcePushDialog}
  <ConfirmDialog
    title="Push Rejected"
    message="The remote branch has commits that would be lost. Do you want to force push? This will overwrite the remote branch with your local version."
    confirmLabel="Force Push"
    danger
    onConfirm={handleForcePushConfirm}
    onCancel={handleForcePushCancel}
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

  /* Primary action button — circular icon-only */
  .primary-action-container {
    display: flex;
    align-items: center;
    overflow: hidden;
  }

  .primary-action-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: var(--bg-elevated);
    border: none;
    border-radius: 50%;
    color: var(--text-base);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .primary-action-button:hover {
    background: var(--bg-hover);
  }

  .primary-action-button.running {
    background: var(--bg-hover);
    color: var(--text-muted);
  }

  .primary-action-button.running:hover {
    background: var(--bg-elevated);
  }

  .primary-action-button.completed {
    background: var(--bg-hover);
    color: var(--status-added);
  }

  .primary-action-button.failed {
    background: var(--bg-hover);
    color: var(--ui-danger);
  }

  .primary-action-button :global(svg) {
    flex-shrink: 0;
    width: 14px;
    height: 14px;
  }

  /* Running actions */
  .running-action-container {
    display: flex;
    align-items: center;
    gap: 4px;
    overflow: hidden;
  }

  .running-action-container.fading {
    opacity: 0;
    transform: scale(0.95);
    transition:
      opacity 0.3s ease,
      transform 0.3s ease;
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
    white-space: nowrap;
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease;
  }

  .running-action-button:hover {
    background: var(--bg-hover);
    border-color: var(--border-focus);
  }

  .running-action-button.completed {
    border-color: var(--status-added);
    color: var(--status-added);
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

  /* PR button */
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
    color: var(--text-muted);
    border-color: var(--border-subtle);
  }

  .pr-btn.created:hover {
    color: var(--text-primary);
    border-color: var(--border-muted);
    background: var(--bg-hover);
  }

  .pr-btn.created :global(svg) {
    color: var(--text-muted);
  }

  .pr-btn.created:hover :global(svg) {
    color: var(--text-primary);
  }

  .pr-btn.pushing {
    color: var(--text-muted);
    border-color: var(--border-muted);
    cursor: default;
  }

  .pr-btn.merged :global(svg) {
    color: var(--status-added);
  }

  .pr-btn :global(svg) {
    flex-shrink: 0;
  }

  /* PR status indicator circle */
  .pr-status-indicator {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
    margin-left: 2px;
  }

  .pr-status-indicator.success {
    background-color: var(--status-added, #4ade80);
  }

  .pr-status-indicator.warning {
    background-color: var(--status-modified, #fb923c);
  }

  .pr-status-indicator.error {
    background-color: var(--ui-danger, #ef4444);
  }

  .pr-status-indicator.neutral {
    background-color: var(--text-faint, #64748b);
  }

  .pr-status-indicator.pending {
    background-color: var(--text-muted, #94a3b8);
    animation: pulse 2s cubic-bezier(0.4, 0, 0.6, 1) infinite;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.5;
    }
  }
</style>
