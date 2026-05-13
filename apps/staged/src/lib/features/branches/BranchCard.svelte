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
  import {
    FileDiff,
    AlertCircle,
    Cloud,
    Trash2,
    GitPullRequest,
    GitPullRequestClosed,
    GitPullRequestDraft,
  } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import { listenToEvent, type UnlistenFn } from '../../transport';
  import { subscribeDragDrop } from './dragDrop';
  import type {
    Branch,
    BranchGitState,
    BranchTimeline as BranchTimelineData,
    HashtagItem,
    ProjectRepo,
    SessionStatusPayload,
    WorkspaceStatus,
  } from '../../types';
  import * as commands from '../../api/commands';
  import BranchTimeline from '../timeline/BranchTimeline.svelte';
  import ImageViewerModal from '../timeline/ImageViewerModal.svelte';
  import { countUserComments, shouldWarnBeforeDeletingReview } from '../timeline/reviewState';
  import DiffModal from '../diff/DiffModal.svelte';
  import SessionModal from '../sessions/SessionModal.svelte';
  import NewSessionModal from '../sessions/NewSessionModal.svelte';
  import NoteModal from '../notes/NoteModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import {
    fileNameFromPath,
    formatBaseBranch,
    isMaybeTextFile,
    isImageFile,
  } from './branchCardHelpers';
  import BranchCardHeaderInfo from './BranchCardHeaderInfo.svelte';
  import BranchCardActionsBar from './BranchCardActionsBar.svelte';
  import BranchCardPrButton from './BranchCardPrButton.svelte';
  import BranchCardSessionManager from './BranchCardSessionManager.svelte';
  import ReasonBanner from './ReasonBanner.svelte';
  import RemoteWorkspaceStatusBadge from './RemoteWorkspaceStatusBadge.svelte';
  import RemoteWorkspaceStatusView from './RemoteWorkspaceStatusView.svelte';
  import { branchTimelineReadyKey } from './branchTimelineReady';
  import { alerts } from '../../shared/alerts.svelte';
  import { aggregateProjectPrStatus } from '../../shared/utils';
  import { timelineToHashtagItems, projectNotesToHashtagItems } from '../sessions/hashtagItems';
  import { sessionRegistry } from '../../stores/sessionRegistry.svelte';
  import { getPreferredAgent } from '../settings/preferences.svelte';
  import { agentState, REMOTE_AGENTS } from '../agents/agent.svelte';
  import type { WorktreeChangesPreview } from '../../commands';

  interface Props {
    branch: Branch;
    repoLabel?: ProjectRepo | null;
    projectName?: string;
    deleting?: boolean;
    worktreeError?: string;
    workspaceError?: string;
    onDelete?: () => void;
    onRename?: (branchName: string) => void;
    onRetryWorktree?: () => void;
    onDismissReason?: (projectRepoId: string) => void;
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
    onDismissReason,
  }: Props = $props();

  // Determine if this is a local or remote branch
  const isLocal = $derived(branch.branchType === 'local');
  const isRemote = $derived(branch.branchType === 'remote');
  const remoteWorkspaceStatus = $derived(branch.workspaceStatus);
  const prStatus = $derived(aggregateProjectPrStatus([branch]));

  function cloudStatusClass(status: WorkspaceStatus | null): string {
    switch (status) {
      case 'running':
        return 'cloud-running';
      case 'starting':
        return 'cloud-starting';
      case 'error':
        return 'cloud-error';
      case 'stopped':
      case 'suspended':
      default:
        return 'cloud-inactive';
    }
  }

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
  /** True while a background git-state refresh (fetch + ref comparison) is in flight. */
  let refreshingGitState = $state(false);
  let error = $state<string | null>(null);
  let pullingOrigin = $state(false);
  let discardingWorktreeChanges = $state(false);
  let showBranchDiff = $state(false);
  let showWorktreeDiff = $state(false);
  let loadedTimelineKey = $state<string | null>(null);
  type TimelineFullReview = NonNullable<Awaited<ReturnType<typeof commands.getReview>>>;
  type TimelineReviewDetails = {
    commitSha: string;
    scope: 'branch' | 'commit';
    comments: number;
    annotations: number;
    warnings: number;
    userComments: number;
  };
  let timelineReviewDetailsById = $state<Record<string, TimelineReviewDetails>>({});

  // Hashtag items for rendering #type:id badges in timeline titles.
  // Timeline-derived items are computed reactively so badges update when timeline refreshes.
  // Project notes are loaded async separately (they don't change during note generation).
  let timelineHashtagItems = $derived(timeline ? timelineToHashtagItems(timeline) : []);
  let projectNoteHashtagItems = $state<HashtagItem[]>([]);
  $effect(() => {
    const projectId = branch.projectId;
    if (!projectId) {
      projectNoteHashtagItems = [];
      return;
    }
    let stale = false;
    commands.listProjectNotes(projectId).then((notes) => {
      if (stale) return;
      projectNoteHashtagItems = projectNotesToHashtagItems(notes);
    });
    return () => {
      stale = true;
    };
  });
  let hashtagItems = $derived([...timelineHashtagItems, ...projectNoteHashtagItems]);
  let reviewDetailsLoadVersion = 0;
  let reviewDiffTarget = $state<{
    commitSha: string;
    scope: 'branch' | 'commit';
    reviewId: string;
  } | null>(null);

  /** True when the branch is still provisioning (local worktree or remote workspace). */
  let isProvisioning = $derived(
    (isLocal && !branch.worktreePath && !worktreeError) ||
      (isRemote && remoteWorkspaceStatus === 'starting')
  );

  /** True during provisioning OR the gap between worktree-ready and timeline-loaded. */
  let isSettingUp = $derived(
    isProvisioning || (isLocal && !!branch.worktreePath && !timeline && !error)
  );

  /** Empty timeline used during provisioning so the action buttons render. */
  const emptyTimeline: BranchTimelineData = {
    commits: [],
    notes: [],
    reviews: [],
    images: [],
    gitState: null,
  };

  // =========================================================================
  // Worktree setup progress (event-driven phases)
  // =========================================================================

  let setupPhase: string | undefined = $state(undefined);
  let setupDetail: string | null = $state(null);

  $effect(() => {
    let cancelled = false;

    const eventNames = ['worktree-setup-progress', 'workspace-setup-progress'] as const;
    const unlisteners: (() => void)[] = [];

    for (const eventName of eventNames) {
      listenToEvent<{ branchId: string; phase: string; detail: string | null }>(
        eventName,
        (payload) => {
          if (payload.branchId === branch.id) {
            setupPhase = payload.phase;
            setupDetail = payload.detail;
          }
        }
      ).then((fn) => {
        if (cancelled) fn();
        else unlisteners.push(fn);
      });
    }

    return () => {
      cancelled = true;
      for (const fn of unlisteners) fn();
    };
  });

  // Reset setup state when provisioning completes
  $effect(() => {
    if (branch.worktreePath || (isRemote && remoteWorkspaceStatus === 'running')) {
      setupPhase = undefined;
      setupDetail = null;
    }
  });

  /** Label for the provisioning timeline row, if applicable. */
  let provisioningLabel = $derived.by(() => {
    if (!isSettingUp) return undefined;
    if (isProvisioning) {
      if (isLocal) {
        if (setupPhase) {
          const labels: Record<string, string> = {
            cloning: 'Cloning repository…',
            fetching: 'Fetching latest changes…',
            creating_worktree: 'Creating worktree…',
            running_setup_actions: 'Running setup actions…',
          };
          return labels[setupPhase] ?? 'Setting up…';
        }
        return 'Setting up…';
      }
      return 'Starting workspace…';
    }
    return 'Looking for changes…';
  });

  /** Map blox orchestrator CommandType enum names to display labels. */
  const remoteCommandLabels: Record<string, string> = {
    checkout: 'Git checkout',
    execute_process: 'Executing process',
    project_bootstrap: 'Project bootstrap',
    provision_workspace: 'Provision workspace',
  };

  /** Detail text for the provisioning row (e.g. git progress percentages or step info). */
  let provisioningDetail = $derived.by(() => {
    if (!isProvisioning) return null;
    if (isLocal) return setupDetail;
    // Remote workspace starting
    if (setupDetail && setupPhase) {
      const label = remoteCommandLabels[setupPhase] ?? setupPhase;
      return `${setupDetail} · ${label}`;
    }
    return setupDetail;
  });

  /** True when the branch has at least one finalized commit (code changes vs base). */
  let hasCodeChanges = $derived(timeline?.commits.some((c) => c.sha) ?? false);

  /** True when the branch has at least one queued or running session. */
  function hasActiveSessions(tl: NonNullable<typeof timeline>): boolean {
    return (
      tl.commits.some((c) => c.sessionStatus === 'queued' || c.sessionStatus === 'running') ||
      tl.notes.some((n) => n.sessionStatus === 'queued' || n.sessionStatus === 'running') ||
      tl.reviews.some(
        (r) => !r.isAuto && (r.sessionStatus === 'queued' || r.sessionStatus === 'running')
      )
    );
  }
  let commandPipelinePending = $state(false);
  let branchSessionBusy = $derived(timeline ? hasActiveSessions(timeline) : false);

  async function startBranchCommandPipeline(
    kind: 'rebase' | 'squash',
    rebaseTarget?: 'base' | 'origin'
  ) {
    if (commandPipelinePending || branchSessionBusy) return;
    commandPipelinePending = true;
    const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
    const provider = getPreferredAgent(agents) ?? undefined;
    const pendingKey = `pipeline-${kind}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    try {
      let sessionId: string;
      if (kind === 'rebase') {
        sessionId = await commands.rebaseBranch(branch.id, provider, rebaseTarget);
      } else {
        sessionId = await commands.squashCommits(branch.id, provider);
      }
      // Add a pending session item so the session stub appears instantly
      // instead of waiting for the full timeline refresh.
      const title = kind === 'rebase' ? 'Rebasing…' : 'Squashing…';
      sessionMgr.pendingSessionItems = [
        ...sessionMgr.pendingSessionItems,
        { key: pendingKey, type: 'pending-commit', title, sessionId },
      ];
      await loadTimeline();
    } catch (e) {
      notifyError(kind === 'rebase' ? 'Rebase failed' : 'Squash failed', e);
    } finally {
      commandPipelinePending = false;
    }
  }

  // Compute suggested prefill prompts from the latest visible timeline entry.
  // Only used when the user hasn't typed a draft yet.
  let suggestedPrefill = $derived.by(() => {
    const empty = { commit: '', note: '', commitRef: '', noteRef: '' };
    if (!timeline) return empty;

    // Don't prefill when there are active (queued or running) sessions — the user
    // should wait for those to complete before starting new work.
    if (hasActiveSessions(timeline)) return empty;

    // Find the latest completed item across commits, notes, and reviews
    type Candidate =
      | { kind: 'review'; id: string; commentCount: number; timestamp: number }
      | {
          kind: 'note';
          id: string;
          title: string;
          timestamp: number;
          suggestedNextCommitStep: string | null;
          suggestedNextNoteStep: string | null;
        };

    const candidates: Candidate[] = [];

    for (const review of timeline.reviews) {
      if (review.isAuto) continue;
      const ts = Math.floor((review.completedAt ?? review.createdAt) / 1000);
      candidates.push({
        kind: 'review',
        id: review.id,
        commentCount: review.commentCount,
        timestamp: ts,
      });
    }

    for (const note of timeline.notes) {
      const ts = Math.floor((note.completedAt ?? note.createdAt) / 1000);
      candidates.push({
        kind: 'note',
        id: note.id,
        title: note.title,
        timestamp: ts,
        suggestedNextCommitStep: note.suggestedNextCommitStep,
        suggestedNextNoteStep: note.suggestedNextNoteStep,
      });
    }

    // We also need commits so we can tell if the latest item overall is a commit
    // (in which case we return blank).
    type AnyCandidate = Candidate | { kind: 'commit'; timestamp: number };
    const all: AnyCandidate[] = [...candidates];
    for (const commit of timeline.commits) {
      if (!commit.sha) continue; // skip pending
      all.push({ kind: 'commit', timestamp: commit.timestamp });
    }

    if (all.length === 0) return empty;

    all.sort((a, b) => b.timestamp - a.timestamp);
    const latest = all[0];

    // If latest item is a note with suggested next steps, use them
    if (
      latest.kind === 'note' &&
      (latest.suggestedNextCommitStep || latest.suggestedNextNoteStep)
    ) {
      const ref = `Re: #note:${latest.id}`;
      return {
        commit: latest.suggestedNextCommitStep ?? '',
        note: latest.suggestedNextNoteStep ?? '',
        commitRef: latest.suggestedNextCommitStep ? ref : '',
        noteRef: latest.suggestedNextNoteStep ? ref : '',
      };
    }

    // Fallback to existing heuristics for reviews and notes without suggestions
    if (latest.kind === 'review' && latest.commentCount > 0) {
      return {
        commit: 'Resolve code review comments',
        note: '',
        commitRef: `Re: #review:${latest.id}`,
        noteRef: '',
      };
    }
    if (latest.kind === 'note' && latest.title.toLowerCase().includes('plan')) {
      return {
        commit: 'Implement plan',
        note: '',
        commitRef: `Re: #note:${latest.id}`,
        noteRef: '',
      };
    }
    if (latest.kind === 'note' && latest.title.toLowerCase().endsWith(' log')) {
      return {
        commit: 'Look for any issues.',
        note: '',
        commitRef: `Re: #note:${latest.id}`,
        noteRef: '',
      };
    }

    return empty;
  });

  // Compute next-step suggestions for a note. Called once when the note modal
  // is opened so the result is static and doesn't cause DOM churn from polling.
  function computeNoteNextSteps(
    noteId: string
  ): { commitStep: string | null; noteStep: string | null } | null {
    if (!timeline) return null;
    const note = timeline.notes.find((n) => n.id === noteId);
    if (!note) return null;
    if (!note.suggestedNextCommitStep && !note.suggestedNextNoteStep) return null;

    return {
      commitStep: note.suggestedNextCommitStep,
      noteStep: note.suggestedNextNoteStep,
    };
  }

  // Commit diff modal (opened by clicking a commit in the timeline)
  let commitDiffSha = $state<string | null>(null);

  // Note modal (opened by clicking a note in the timeline)
  let openNote = $state<{
    noteId: string;
    title: string;
    content: string;
    sessionId?: string;
    nextSteps?: { commitStep: string | null; noteStep: string | null } | null;
  } | null>(null);

  // Image viewer modal (opened by clicking an image in the timeline)
  let viewImageId = $state<string | null>(null);
  let viewImageFilename = $state<string>('');
  let deletingImageIds = $state<Set<string>>(new Set());
  let deletingCommitKeys = $state<Set<string>>(new Set());
  let timelineDeletingItems = $derived([
    ...[...deletingImageIds].map((id) => ({ type: 'image' as const, id })),
    ...[...deletingCommitKeys].map((id) => ({ type: 'commit' as const, id })),
  ]);

  /** Session IDs that were just pruned from pendingItems because the real timeline item arrived. */
  let prunedSessionIds = $state<Set<string>>(new Set());

  // Confirm delete dialog
  let confirmDelete = $state<{
    title: string;
    message: string;
    confirmLabel?: string;
    onConfirm: () => void;
  } | null>(null);

  // =========================================================================
  // Session manager (reactive .svelte.ts module)
  // =========================================================================

  const sessionMgr = new BranchCardSessionManager({
    getBranch: () => branch,
    getIsRemote: () => isRemote,
    loadTimeline: (opts) => loadTimeline(opts),
    getTimeline: () => timeline,
    setTimeline: (tl) => {
      timeline = tl;
    },
  });

  let requestedTimelineKey: string | null = null;
  let timelineLoadVersion = 0;
  let revalidationVersion = 0;

  function isCurrentTimelineLoad(loadVersion: number, timelineKey: string): boolean {
    return loadVersion === timelineLoadVersion && branchTimelineReadyKey(branch) === timelineKey;
  }

  /**
   * Apply a cached timeline immediately and, if a `fresh` promise is provided,
   * set up revalidation handlers guarded by `revalidationVersion` so stale
   * responses are discarded.
   *
   * Shared by the synchronous-hydration block (below) and the `isInitialLoad`
   * path inside `loadTimeline()`.
   */
  function applyCachedTimeline(
    cached: BranchTimelineData,
    fresh: Promise<BranchTimelineData> | null,
    timelineKey: string
  ) {
    timeline = cached;
    loadedTimelineKey = timelineKey;
    loading = false;
    prunedSessionIds = sessionMgr.prunePendingSessionItems(cached);
    if (fresh) {
      const version = ++revalidationVersion;
      fresh
        .then((next) => {
          if (version !== revalidationVersion || branchTimelineReadyKey(branch) !== timelineKey) {
            return;
          }
          error = null;
          timeline = next;
          loadedTimelineKey = timelineKey;
          prunedSessionIds = sessionMgr.prunePendingSessionItems(next);
          void loadTimelineReviewDetails(next.reviews);
        })
        .catch((e) => {
          if (version !== revalidationVersion || branchTimelineReadyKey(branch) !== timelineKey) {
            return;
          }
          error = e instanceof Error ? e.message : String(e);
        });
    } else {
      void loadTimelineReviewDetails(cached.reviews);
    }

    // Kick off a background git-state refresh (TTL-gated fetch).
    refreshingGitState = true;
    commands.refreshBranchGitState(branch.id).catch(() => {
      refreshingGitState = false;
    });
  }

  // Synchronously hydrate timeline from cache so isSettingUp is never true
  // on remount (e.g. project switch). This prevents the loading flash and
  // the slide-in animation for already-cached rows.
  {
    // svelte-ignore state_referenced_locally
    const initBranch = branch;
    untrack(() => {
      const key = branchTimelineReadyKey(initBranch);
      if (key) {
        const { cached, fresh } = commands.getBranchTimelineWithRevalidation(initBranch.id);
        if (cached) {
          applyCachedTimeline(cached, fresh, key);
        }
      }
    });
  }

  /** Number of finalized commits on this branch. */
  let commitCount = $derived(timeline?.commits.filter((c) => c.sha).length ?? 0);

  function gitIdentityWarning(state: BranchGitState | null | undefined): string | null {
    if (!state) return null;
    if (state.fetch.status === 'failed') return null; // branch info unreliable when fetch failed
    if (state.detachedHead) return 'Detached HEAD';
    if (!state.expectedBranchMatches) {
      return state.currentBranch ? `Checked out ${state.currentBranch}` : 'Wrong branch';
    }
    return null;
  }

  let branchIdentityWarning = $derived(gitIdentityWarning(timeline?.gitState));
  let gitUnsafeActionsDisabled = $derived(!!branchIdentityWarning);
  let branchCommandDisabledReason = $derived(
    branchIdentityWarning ??
      (commandPipelinePending
        ? 'Command in progress'
        : branchSessionBusy
          ? 'Session in progress'
          : null)
  );

  // =========================================================================
  // PR button ref
  // =========================================================================
  let prButton = $state<ReturnType<typeof BranchCardPrButton> | undefined>();

  // =========================================================================
  // Event listeners
  // =========================================================================

  let unlistenStatus: UnlistenFn | null = null;

  $effect(() => {
    const branchId = branch.id;

    listenToEvent<SessionStatusPayload>('session-status-changed', (payload) => {
      const { sessionId: eventSessionId, status, branchId: eventBranchId, isAutoReview } = payload;
      if (status === 'completed' || status === 'error' || status === 'cancelled') {
        // If this is the auto review session completing, just clear tracking
        if (eventSessionId === sessionMgr.autoReviewSessionId) {
          sessionMgr.autoReviewSessionId = null;
          return;
        }

        // Clear push/force-push session tracking on terminal status only
        // (guarded by the outer completed/error/cancelled check above)
        if (eventSessionId === pushSessionId) {
          pushSessionId = null;
        }
        if (eventSessionId === forcePushSessionId) {
          forcePushSessionId = null;
        }

        // Skip normal completion handling for any auto review session
        if (isAutoReview) {
          return;
        }

        // Skip reload for the adopted auto-review session completing —
        // the timeline was already updated optimistically during adoption.
        if (eventSessionId === sessionMgr.adoptedSessionId) {
          sessionMgr.adoptedSessionId = null;
          return;
        }

        // Only reload if this session belongs to our branch
        if (eventBranchId && eventBranchId !== branchId) return;

        commands.invalidateBranchTimeline(branch.id);
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
        // Refresh the timeline so the pending note/commit stub appears immediately.
        // Skip if a session start is in-flight (pending item has no sessionId yet),
        // because startBranchSessionWithPendingItem will call loadTimeline after
        // it gets the sessionId — otherwise pruning can't match the pending item
        // and both the pending and real items briefly render simultaneously.
        if (!isAutoReview && !sessionMgr.isSessionStartPending) {
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

  // Listen for git-state-updated events emitted by refresh_branch_git_state.
  // Merges the fresh git state into the existing timeline without a full reload.
  let unlistenGitState: UnlistenFn | null = null;
  $effect(() => {
    const branchId = branch.id;

    listenToEvent<{ branchId: string; gitState: BranchGitState }>(
      'git-state-updated',
      (payload) => {
        if (payload.branchId !== branchId) return;
        if (timeline) {
          timeline = { ...timeline, gitState: payload.gitState };
        }
        refreshingGitState = false;
      }
    ).then((unlisten) => {
      unlistenGitState = unlisten;
    });

    return () => {
      unlistenGitState?.();
    };
  });

  // Load timeline when a branch becomes timeline-ready
  $effect(() => {
    const timelineKey = branchTimelineReadyKey(branch);
    if (!timelineKey) return;
    if (timelineKey === loadedTimelineKey || timelineKey === requestedTimelineKey) return;

    void loadTimeline({ timelineKey });
  });

  // Re-fetch timeline when the cache is invalidated (e.g. after project-setup-progress)
  $effect(() => {
    const handler = (e: Event) => {
      const { branchIds } = (e as CustomEvent<{ branchIds: string[] }>).detail;
      const timelineKey = branchTimelineReadyKey(branch);
      if (branchIds.includes(branch.id) && timelineKey) {
        void loadTimeline({ force: true });
      }
    };
    window.addEventListener('timeline-invalidated', handler);
    return () => window.removeEventListener('timeline-invalidated', handler);
  });

  async function loadTimeline({
    timelineKey = branchTimelineReadyKey(branch),
    force = false,
  }: { timelineKey?: string | null; force?: boolean } = {}) {
    if (!timelineKey) return;

    const loadVersion = ++timelineLoadVersion;
    requestedTimelineKey = timelineKey;
    const isInitialLoad = !timeline || loadedTimelineKey !== timelineKey;
    error = null;
    // Cancel any in-flight revalidation so it can't overwrite fresher data
    revalidationVersion++;

    try {
      if (isInitialLoad) {
        if (!force) {
          const { cached, fresh } = commands.getBranchTimelineWithRevalidation(branch.id);
          if (cached) {
            if (isCurrentTimelineLoad(loadVersion, timelineKey)) {
              applyCachedTimeline(cached, fresh, timelineKey);
            }
            return;
          }
        }
        // No cache — show loading spinner as before
        loading = true;
      }

      const nextTimeline = await commands.getBranchTimeline(branch.id, {
        force: force || !isInitialLoad,
      });
      if (!isCurrentTimelineLoad(loadVersion, timelineKey)) return;
      timeline = nextTimeline;
      loadedTimelineKey = timelineKey;
      prunedSessionIds = sessionMgr.prunePendingSessionItems(nextTimeline);
      void loadTimelineReviewDetails(nextTimeline.reviews);
    } catch (e) {
      if (!isCurrentTimelineLoad(loadVersion, timelineKey)) return;
      error = e instanceof Error ? e.message : String(e);
    } finally {
      if (requestedTimelineKey === timelineKey) {
        requestedTimelineKey = null;
      }
      if (isCurrentTimelineLoad(loadVersion, timelineKey)) {
        loading = false;
      }
    }

    // Kick off a background git-state refresh (TTL-gated fetch).
    // The result arrives via the `git-state-updated` event listener above.
    refreshingGitState = true;
    commands.refreshBranchGitState(branch.id).catch(() => {
      refreshingGitState = false;
    });
  }

  function getTimelineReviewDetails(fullReview: TimelineFullReview): TimelineReviewDetails {
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

    return {
      commitSha: fullReview.commitSha,
      scope: fullReview.scope,
      comments,
      annotations,
      warnings,
      userComments: countUserComments(fullReview.comments),
    };
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

          return { id: review.id, details: getTimelineReviewDetails(fullReview) };
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

  /** Look up note info from timeline data by session ID (for cross-modal navigation). */
  function findNoteForSession(
    sessionId: string
  ): { id: string; title: string; content: string } | null {
    const note = timeline?.notes.find((n) => n.sessionId === sessionId && n.content?.trim());
    if (!note) return null;
    return { id: note.id, title: note.title, content: note.content };
  }

  function handleCommitClick(sha: string) {
    commitDiffSha = sha;
  }

  function handleNoteClick(noteId: string, title: string, content: string, sessionId?: string) {
    openNote = { noteId, title, content, sessionId, nextSteps: computeNoteNextSteps(noteId) };
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

  async function handlePullOrigin() {
    if (pullingOrigin) return;
    pullingOrigin = true;
    try {
      await commands.pullBranchFastForward(branch.id);
      await loadTimeline();
    } catch (e) {
      notifyError('Pull failed', e);
    } finally {
      pullingOrigin = false;
    }
  }

  let pushSessionId = $state<string | null>(null);
  let pushingOrigin = $derived(!!pushSessionId);

  async function handlePushOrigin() {
    if (pushSessionId || commandPipelinePending || branchSessionBusy) return;
    const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
    const provider = getPreferredAgent(agents) ?? undefined;
    try {
      pushSessionId = await commands.pushBranch(branch.id, provider, false);
    } catch (e) {
      pushSessionId = null;
      notifyError('Push failed', e);
    }
  }

  function openPushSession() {
    if (pushSessionId) {
      sessionMgr.openSessionId = pushSessionId;
    }
  }

  let showForcePushDialog = $state(false);

  function handleForcePush() {
    showForcePushDialog = true;
  }

  let forcePushSessionId = $state<string | null>(null);
  let forcePushingOrigin = $derived(!!forcePushSessionId);

  async function confirmForcePush() {
    if (forcePushSessionId || commandPipelinePending || branchSessionBusy) {
      // Another operation is in progress — keep the dialog open so the user
      // understands why the action didn't proceed.
      return;
    }
    showForcePushDialog = false;
    const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
    const provider = getPreferredAgent(agents) ?? undefined;
    try {
      forcePushSessionId = await commands.pushBranch(branch.id, provider, true);
    } catch (e) {
      forcePushSessionId = null;
      notifyError('Force push failed', e);
    }
  }

  function openForcePushSession() {
    if (forcePushSessionId) {
      sessionMgr.openSessionId = forcePushSessionId;
    }
  }

  function formatDiscardPreview(preview: WorktreeChangesPreview): string {
    const sections: string[] = [];
    if (preview.revertPaths.length > 0) {
      sections.push(
        `Revert tracked changes:\n${preview.revertPaths.map((path) => `- ${path}`).join('\n')}`
      );
    }
    if (preview.removePaths.length > 0) {
      sections.push(
        `Remove untracked/new files:\n${preview.removePaths.map((path) => `- ${path}`).join('\n')}`
      );
    }
    return `This will discard uncommitted worktree changes.\n\n${sections.join('\n\n')}`;
  }

  async function handleDiscardWorktreeChanges() {
    if (discardingWorktreeChanges) return;

    let preview: WorktreeChangesPreview;
    try {
      preview = await commands.getWorktreeChangesPreview(branch.id);
    } catch (e) {
      notifyError('Could not inspect changes', e);
      return;
    }

    if (preview.conflictedPaths.length > 0) {
      alerts.show({
        tone: 'error',
        title: 'Conflicts need manual recovery',
        message: preview.conflictedPaths.join('\n'),
        durationMs: 0,
      });
      return;
    }

    if (preview.revertPaths.length === 0 && preview.removePaths.length === 0) {
      await loadTimeline();
      return;
    }

    const doDiscard = async () => {
      confirmDelete = null;
      discardingWorktreeChanges = true;
      try {
        await commands.discardWorktreeChanges(branch.id, preview);
        commands.invalidateBranchTimeline(branch.id);
        await loadTimeline();
      } catch (e) {
        notifyError('Discard failed', e);
      } finally {
        discardingWorktreeChanges = false;
      }
    };

    confirmDelete = {
      title: 'Discard Changes',
      message: formatDiscardPreview(preview),
      confirmLabel: 'Discard',
      onConfirm: doDiscard,
    };
  }

  function handleDeleteCommit(sha: string, sessionId?: string, opts?: { altKey: boolean }) {
    const doDelete = async () => {
      confirmDelete = null;
      deletingCommitKeys = new Set([...deletingCommitKeys, sha]);
      try {
        await commands.deleteCommit(branch.id, sha, !!sessionId);
        await loadTimeline();
      } catch (e) {
        console.error('Failed to delete commit:', e);
        notifyError('Failed to delete commit', e);
      } finally {
        deletingCommitKeys = new Set([...deletingCommitKeys].filter((k) => k !== sha));
      }
    };
    if (opts?.altKey) {
      doDelete();
      return;
    }
    confirmDelete = {
      title: 'Delete Commit',
      message:
        'This will reset the branch to the parent commit, removing this commit and its changes.' +
        (sessionId ? ' The linked session will also be deleted.' : ''),
      onConfirm: doDelete,
    };
  }

  function handleDeleteNote(noteId: string, sessionId?: string, opts?: { altKey: boolean }) {
    const doDelete = async () => {
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
        if (sessionId) {
          sessionRegistry.cleanupSession(sessionId);
        }
        loadTimeline();
        // Drain the next queued session now that this one has been removed.
        commands
          .drainQueuedSessions(branch.id)
          .catch((e) => console.error('Failed to drain queued sessions:', e));
      } catch (e) {
        console.error('Failed to delete note:', e);
        notifyError('Failed to delete note', e);
      }
    };
    if (opts?.altKey) {
      doDelete();
      return;
    }
    confirmDelete = {
      title: 'Delete Note',
      message:
        'Are you sure you want to delete this note?' +
        (sessionId ? ' The linked session will also be deleted.' : ''),
      onConfirm: doDelete,
    };
  }

  function handleDeleteReview(reviewId: string, sessionId?: string, opts?: { altKey: boolean }) {
    const doDelete = async () => {
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
        if (sessionId) {
          sessionRegistry.cleanupSession(sessionId);
        }
        loadTimeline();
        // Drain the next queued session now that this one has been removed.
        commands
          .drainQueuedSessions(branch.id)
          .catch((e) => console.error('Failed to drain queued sessions:', e));
      } catch (e) {
        console.error('Failed to delete review:', e);
        notifyError('Failed to delete review', e);
      }
    };
    const showConfirmDelete = () => {
      confirmDelete = {
        title: 'Delete Review',
        message:
          'Are you sure you want to delete this review and all its comments?' +
          (sessionId ? ' The linked session will also be deleted.' : ''),
        onConfirm: doDelete,
      };
    };
    const decideDelete = async () => {
      let details = timelineReviewDetailsById[reviewId] ?? null;
      try {
        if (!details) {
          const review = await commands.getReview(reviewId);
          if (review) {
            details = getTimelineReviewDetails(review);
            timelineReviewDetailsById = { ...timelineReviewDetailsById, [reviewId]: details };
          }
        }
      } catch (e) {
        console.error('Failed to check review before delete:', e);
        showConfirmDelete();
        return;
      }

      // Conservative fallback: if we couldn't load review details, always warn
      if (!details) {
        showConfirmDelete();
        return;
      }

      const shouldWarn = shouldWarnBeforeDeletingReview({
        review: details,
        commits: timeline?.commits ?? [],
        userCommentCount: details.userComments,
      });
      if (shouldWarn) {
        showConfirmDelete();
      } else {
        doDelete();
      }
    };
    if (opts?.altKey) {
      doDelete();
      return;
    }
    void decideDelete();
  }

  async function handleDeletePendingCommit(commitId: string, sessionId?: string) {
    deletingCommitKeys = new Set([...deletingCommitKeys, commitId]);
    try {
      if (sessionId) {
        try {
          await commands.cancelSession(sessionId);
        } catch {
          // Session may already be finished, that's fine
        }
      }
      await commands.deletePendingCommit(commitId, !!sessionId);
      if (sessionId) {
        sessionRegistry.cleanupSession(sessionId);
      }
      await loadTimeline();
      // Drain the next queued session now that this one has been removed.
      commands
        .drainQueuedSessions(branch.id)
        .catch((e) => console.error('Failed to drain queued sessions:', e));
    } catch (e) {
      console.error('Failed to delete pending commit:', e);
      notifyError('Failed to delete pending commit', e);
    } finally {
      deletingCommitKeys = new Set([...deletingCommitKeys].filter((k) => k !== commitId));
    }
  }

  function handleImageClick(imageId: string) {
    const image = timeline?.images.find((img) => img.id === imageId);
    if (image) {
      viewImageId = imageId;
      viewImageFilename = image.filename;
    }
  }

  function handleDeleteImage(imageId: string, opts?: { altKey: boolean }) {
    const doDelete = async () => {
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
    };
    if (opts?.altKey) {
      doDelete();
      return;
    }
    confirmDelete = {
      title: 'Delete Image',
      message: 'Are you sure you want to delete this image?',
      onConfirm: doDelete,
    };
  }

  // =========================================================================
  // Repo reason banner
  // =========================================================================

  async function handleDismissReason() {
    if (branch.projectRepoId) {
      try {
        await commands.clearProjectRepoReason(branch.projectRepoId);
        onDismissReason?.(branch.projectRepoId);
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
    const textPaths = paths.filter(isMaybeTextFile);
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
            const reason = e instanceof Error ? e.message : typeof e === 'string' ? e : null;
            const detail = reason ?? 'it may be a binary file';
            alerts.error(`Could not read "${fileNameFromPath(filePath)}" \u2014 ${detail}`);
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
  {:else if isLocal && !branch.worktreePath && worktreeError}
    <div class="card-header">
      <BranchCardHeaderInfo
        branchName={branch.branchName}
        {repoLabel}
        baseBranch={formatBaseBranch(branch.baseBranch)}
      />
      <div class="header-actions">
        <button class="more-button" onclick={() => onDelete?.()} title="Delete branch">
          <Trash2 size={16} />
        </button>
      </div>
    </div>
    <div class="card-content">
      <div class="worktree-error">
        <div class="worktree-error-message">
          <AlertCircle size={14} />
          <span>Failed to create worktree: {worktreeError}</span>
        </div>
        <button class="worktree-retry-btn" onclick={() => onRetryWorktree?.()}> Retry </button>
      </div>
    </div>
  {:else}
    <div class="card-header">
      {#if isRemote}
        <Cloud size={14} class="header-icon {cloudStatusClass(remoteWorkspaceStatus)}" />
      {:else if prStatus === 'merged'}
        <GitPullRequest size={14} class="header-icon pr-status-merged" />
      {:else if prStatus === 'checks_failing'}
        <GitPullRequest size={14} class="header-icon pr-status-checks-failing" />
      {:else if prStatus === 'open'}
        <GitPullRequest size={14} class="header-icon" />
      {:else if prStatus === 'closed'}
        <GitPullRequestClosed size={14} class="header-icon" />
      {:else if prStatus === 'conflict'}
        <GitPullRequestClosed size={14} class="header-icon pr-status-conflict" />
      {:else}
        <GitPullRequestDraft size={14} class="header-icon pr-status-draft" />
      {/if}
      <BranchCardHeaderInfo
        branchName={branch.branchName}
        {repoLabel}
        baseBranch={isRemote
          ? (branch.workspaceName ?? formatBaseBranch(branch.baseBranch))
          : formatBaseBranch(branch.baseBranch)}
        parentAheadCount={refreshingGitState ? 0 : (timeline?.gitState?.base.commitsSinceFork ?? 0)}
        onRebase={branchCommandDisabledReason
          ? undefined
          : () => startBranchCommandPipeline('rebase')}
        rebaseDisabled={!!branchCommandDisabledReason}
        warning={branchIdentityWarning}
        {refreshingGitState}
        fetchError={timeline?.gitState?.fetch.error ?? null}
      />
      <div class="header-actions">
        {#if isRemote && remoteWorkspaceStatus !== 'running' && remoteWorkspaceStatus !== 'starting'}
          <RemoteWorkspaceStatusBadge status={remoteWorkspaceStatus} />
        {/if}
        <BranchCardActionsBar
          {branch}
          {repoLabel}
          {isLocal}
          {isRemote}
          {isSettingUp}
          {remoteWorkspaceStatus}
          {onDelete}
          {onRename}
          onNoteCreated={() => loadTimeline()}
          onRebaseBranch={() => startBranchCommandPipeline('rebase')}
          onSquashCommits={() => startBranchCommandPipeline('squash')}
          newCommitDisabled={sessionMgr.isNewSessionDisabled ||
            commandPipelinePending ||
            branchSessionBusy ||
            gitUnsafeActionsDisabled}
          {commitCount}
        />
      </div>
    </div>

    <div class="card-content">
      <ReasonBanner reason={repoLabel?.reason} onDismiss={handleDismissReason} />
      {#if isRemote && (remoteWorkspaceStatus === 'stopped' || remoteWorkspaceStatus === 'suspended' || remoteWorkspaceStatus === 'error')}
        <RemoteWorkspaceStatusView
          status={remoteWorkspaceStatus}
          {workspaceError}
          fallbackError={error}
        />
      {:else if loading && !isSettingUp}
        <div class="loading">
          <Spinner size={14} />
          <span>Loading...</span>
        </div>
      {:else if error && !timeline}
        <div class="error">
          <span>{error}</span>
          <button class="retry-btn" onclick={() => loadTimeline()}>Retry</button>
        </div>
      {:else if timeline || isSettingUp}
        <BranchTimeline
          timeline={timeline ?? emptyTimeline}
          repoDir={branch.worktreePath}
          {hashtagItems}
          pendingDropNotes={isLocal ? pendingDropNotes : undefined}
          pendingItems={sessionMgr.pendingSessionItems}
          {prunedSessionIds}
          {error}
          gitActionDisabledReason={branchIdentityWarning}
          onRetry={() => loadTimeline()}
          deletingItems={timelineDeletingItems}
          reviewCommentBreakdown={timelineReviewDetailsById}
          onSessionClick={(sid) => sessionMgr.handleTimelineSessionClick(sid)}
          onResumeClick={(sid) => {
            commands
              .resumeSession(sid, 'Continue where you left off.', undefined, branch.id)
              .then(() => loadTimeline())
              .catch((e) => {
                console.error('Failed to resume session:', e);
                alerts.error(
                  e instanceof Error
                    ? e.message
                    : 'Could not resume the session. Please try again.',
                  'Resume failed'
                );
              });
          }}
          onCommitClick={handleCommitClick}
          onNoteClick={handleNoteClick}
          onReviewClick={handleReviewClick}
          onDeleteCommit={handleDeleteCommit}
          onDeletePendingCommit={handleDeletePendingCommit}
          onDeleteNote={handleDeleteNote}
          onDeleteReview={handleDeleteReview}
          onImageClick={handleImageClick}
          onDeleteImage={handleDeleteImage}
          onStartQueued={() => {
            commands
              .drainQueuedSessions(branch.id)
              .catch((e) => console.error('Failed to drain queued sessions:', e));
          }}
          onNewNote={() => sessionMgr.openNewSession('note')}
          onNewCommit={() => sessionMgr.openNewSession('commit')}
          onNewReview={hasCodeChanges || sessionMgr.hasCommitSessionInProgress
            ? (e) => sessionMgr.openNewSession('review', e)
            : undefined}
          onPullOrigin={handlePullOrigin}
          onPushOrigin={handlePushOrigin}
          onOpenPushSession={pushSessionId ? openPushSession : undefined}
          onRebaseBranch={() => startBranchCommandPipeline('rebase')}
          onRebaseBranchOntoOrigin={() => startBranchCommandPipeline('rebase', 'origin')}
          onForcePush={handleForcePush}
          onOpenForcePushSession={forcePushSessionId ? openForcePushSession : undefined}
          {forcePushingOrigin}
          rebaseBranchDisabledReason={branchCommandDisabledReason}
          onViewWorktreeDiff={isLocal ? () => (showWorktreeDiff = true) : undefined}
          onCommitWorktreeChanges={() =>
            sessionMgr.startOrQueueSession('commit', 'Commit uncommitted changes')}
          onDiscardWorktreeChanges={handleDiscardWorktreeChanges}
          onNewSessionReferring={(ref) => sessionMgr.openNewSessionReferring(ref)}
          newSessionDisabled={sessionMgr.isNewSessionDisabled || gitUnsafeActionsDisabled}
          {pullingOrigin}
          {pushingOrigin}
          {discardingWorktreeChanges}
          {provisioningLabel}
          {provisioningDetail}
        >
          {#snippet footerActions()}
            {#if hasCodeChanges || branch.prNumber}
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
                {#if hasCodeChanges}
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
                {/if}
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
    projectId={branch.projectId}
    commitSha={reviewDiffTarget?.commitSha}
    scope={reviewDiffTarget?.scope ?? 'branch'}
    reviewId={reviewDiffTarget?.reviewId}
    beforeLabel={reviewDiffTarget?.scope === 'commit'
      ? 'parent'
      : formatBaseBranch(branch.baseBranch)}
    afterLabel={reviewDiffTarget?.commitSha
      ? reviewDiffTarget.commitSha.slice(0, 7)
      : branch.branchName}
    commits={timeline?.commits}
    baseBranchLabel={formatBaseBranch(branch.baseBranch)}
    branchLabel={branch.branchName}
    {projectName}
    githubRepo={repoLabel?.headRepo ?? repoLabel?.githubRepo}
    subpath={repoLabel?.subpath}
    onClose={() => {
      showBranchDiff = false;
      reviewDiffTarget = null;
      loadTimeline();
    }}
  />
{/if}

{#if showWorktreeDiff}
  <DiffModal
    branchId={branch.id}
    projectId={branch.projectId}
    scope="worktree"
    readonly
    beforeLabel="HEAD"
    afterLabel="worktree"
    baseBranchLabel={formatBaseBranch(branch.baseBranch)}
    branchLabel={branch.branchName}
    {projectName}
    githubRepo={repoLabel?.headRepo ?? repoLabel?.githubRepo}
    subpath={repoLabel?.subpath}
    onClose={() => {
      showWorktreeDiff = false;
      loadTimeline();
    }}
  />
{/if}

{#if commitDiffSha}
  <DiffModal
    branchId={branch.id}
    projectId={branch.projectId}
    commitSha={commitDiffSha}
    scope="commit"
    beforeLabel="parent"
    afterLabel={commitDiffSha.slice(0, 7)}
    commits={timeline?.commits}
    baseBranchLabel={formatBaseBranch(branch.baseBranch)}
    branchLabel={branch.branchName}
    {projectName}
    githubRepo={repoLabel?.headRepo ?? repoLabel?.githubRepo}
    subpath={repoLabel?.subpath}
    onClose={() => {
      commitDiffSha = null;
      loadTimeline();
    }}
  />
{/if}

{#if openNote}
  <NoteModal
    title={openNote.title}
    content={openNote.content}
    sessionId={openNote.sessionId}
    nextSteps={openNote.nextSteps}
    onClose={() => (openNote = null)}
    onOpenSession={(sid) => {
      openNote = null;
      sessionMgr.openSessionId = sid;
    }}
    onStartSession={(mode, prefill) => {
      const noteRef = openNote?.noteId ? `Re: #note:${openNote.noteId}` : '';
      openNote = null;
      void sessionMgr.startOrQueueSession(mode, noteRef ? `${noteRef}\n${prefill}` : prefill);
    }}
  />
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
  {@const commitPrefillBase = suggestedPrefill.commit}
  {@const notePrefillBase = suggestedPrefill.note}
  {@const commitPrefill = suggestedPrefill.commitRef
    ? `${suggestedPrefill.commitRef}\n${commitPrefillBase}`
    : commitPrefillBase}
  {@const notePrefill = suggestedPrefill.noteRef
    ? `${suggestedPrefill.noteRef}\n${notePrefillBase}`
    : notePrefillBase}
  {@const usePrefill =
    !sessionMgr.draftPrompt &&
    ((sessionMgr.newSessionMode === 'commit' && !!commitPrefillBase) ||
      (sessionMgr.newSessionMode === 'note' && !!notePrefillBase))}
  {@const prefillText =
    sessionMgr.newSessionMode === 'note'
      ? notePrefill
      : sessionMgr.newSessionMode === 'commit'
        ? commitPrefill
        : ''}
  <NewSessionModal
    {branch}
    mode={sessionMgr.newSessionMode}
    {repoLabel}
    initialPrompt={usePrefill ? prefillText : sessionMgr.draftPrompt}
    initialImageIds={sessionMgr.draftImageIds}
    prefilled={usePrefill}
    {commitPrefill}
    {notePrefill}
    remote={isRemote}
    willQueue={sessionMgr.willQueue}
    onClose={(draft) => {
      // Don't persist prefilled text as a draft — it should be re-evaluated
      // each time the dialog opens based on the current timeline state.
      const prompt =
        draft.prompt.trim() === commitPrefill.trim() || draft.prompt.trim() === notePrefill.trim()
          ? ''
          : draft.prompt;
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
    noteInfo={findNoteForSession(sessionMgr.openSessionId)}
    onOpenNote={(noteId, title, content) => {
      const sid = sessionMgr.openSessionId;
      sessionMgr.openSessionId = null;
      openNote = {
        noteId,
        title,
        content,
        sessionId: sid ?? undefined,
        nextSteps: computeNoteNextSteps(noteId),
      };
    }}
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
            prButton.handlePushSessionComplete(session.status, session);
          }
        } catch {
          // Ignore — the polling fallback will catch it
        }
      }
    }}
  />
{/if}

{#if showForcePushDialog}
  <ConfirmDialog
    title="Force Push"
    message="The remote branch has commits that would be lost. Do you want to force push? This will overwrite the remote branch with your local version."
    confirmLabel="Force Push"
    danger
    onConfirm={confirmForcePush}
    onCancel={() => (showForcePushDialog = false)}
  />
{/if}

{#if confirmDelete}
  <ConfirmDialog
    title={confirmDelete.title}
    message={confirmDelete.message}
    confirmLabel={confirmDelete.confirmLabel ?? 'Delete'}
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

  .card-header :global(svg.pr-status-merged) {
    stroke: var(--ui-success);
  }

  .card-header :global(svg.pr-status-conflict) {
    stroke: var(--ui-danger);
  }

  .card-header :global(svg.pr-status-checks-failing) {
    stroke: var(--ui-danger);
  }

  .card-header :global(svg.pr-status-draft) {
    stroke: var(--text-muted);
  }

  .card-header :global(svg.cloud-running) {
    stroke: var(--ui-accent);
  }

  .card-header :global(svg.cloud-starting) {
    stroke: var(--ui-info);
  }

  .card-header :global(svg.cloud-error) {
    stroke: var(--ui-danger);
  }

  .card-header :global(svg.cloud-inactive) {
    stroke: var(--text-muted);
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
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--ui-danger);
    font-size: var(--size-sm);
  }

  .error .retry-btn {
    padding: 2px 10px;
    border-radius: 4px;
    border: 1px solid var(--border-subtle);
    background: none;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
  }

  .error .retry-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
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
