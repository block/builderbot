<!--
  BranchCardPrButton.svelte - PR creation/push/status button

  Handles PR creation, push operations, status polling, and associated
  error/confirmation dialogs.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { toast } from 'svelte-sonner';
  import GitPullRequestCreateArrow from '@lucide/svelte/icons/git-pull-request-create-arrow';
  import GitPullRequestArrow from '@lucide/svelte/icons/git-pull-request-arrow';
  import GitPullRequestDraft from '@lucide/svelte/icons/git-pull-request-draft';
  import GitMerge from '@lucide/svelte/icons/git-merge';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Clock from '@lucide/svelte/icons/clock';
  import Search from '@lucide/svelte/icons/search';
  import Spinner from '../../shared/Spinner.svelte';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { Badge } from '$lib/components/ui/badge';
  import { Button, buttonVariants } from '$lib/components/ui/button';
  import { cn } from '$lib/components/utils';
  import { minuteNow, secondNow } from '../../shared/relativeTime.svelte';
  import type {
    Branch,
    BranchTimeline as BranchTimelineData,
    PrFailedCheck,
    RejectedPrCandidate,
    Session,
  } from '../../types';
  import * as commands from '../../api/commands';
  import {
    classifyCompletedPushSession,
    classifyPipelinePushCompletion,
    classifyPolledSession,
    createPollFailureTracker,
    createQueuedSessionCanceller,
    extractPrNumber,
    extractPrUrl,
    type CompletedPushOutcome,
  } from './branchCardHelpers';
  import { buildPrButtonTitle } from './prButtonTooltip';
  import { derivePrPushAction } from './prButtonGitState';
  import { describePrRejectionReason } from './prDetection';
  import { getPreferredAgent } from '../settings/preferences.svelte';
  import { agentState, REMOTE_AGENTS } from '../agents/agent.svelte';
  import { prStateStore, type PrState } from '../../stores/prState.svelte';
  import { pushStateStore, type PushState } from '../../stores/pushState.svelte';
  import * as prPollingService from '../../services/prPollingService';
  import {
    onBranchPrStatusChanged,
    onBranchPrStatusCleared,
  } from '../../services/branchEventService';

  interface Props {
    branch: Branch;
    isLocal: boolean;
    isRemote: boolean;
    hasCodeChanges: boolean;
    timeline: BranchTimelineData | null;
    showButton?: boolean;
    onOpenSession?: (sessionId: string) => void;
  }

  let {
    branch,
    isLocal,
    isRemote,
    hasCodeChanges,
    timeline,
    showButton = true,
    onOpenSession,
  }: Props = $props();

  export { renderButton };

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
  // =========================================================================
  let storePrState = $derived(prStateStore.getPrState(branch.id));
  let prState = $derived<PrState>(
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
  let cachedPrUrl = $state<string | null>(null);
  let showPrErrorDialog = $state(false);

  // Clean up stale 'creating' entries when branch already has a PR number
  $effect(() => {
    const stored = prStateStore.getPrState(branch.id);
    if (stored && branch.prNumber && stored.state === 'creating') {
      prStateStore.clearPrState(branch.id);
    }
  });

  // PR head SHA — updated from events and branch prop
  let prHeadSha = $state<string | null>(null);
  let prFetchedAt = $state<number | null>(null);

  // Stale-data indicator (set by the centralized polling service)
  let prStatusStale = $state(false);
  let prStatusRefreshing = $state(false);
  let prStatusCleared = $state(false);

  // PR status fields (local state, updated via events)
  let prStatusState = $state<string | null>(null);
  let prStatusChecks = $state<string | null>(null);
  let prStatusReviewDecision = $state<string | null>(null);
  let prStatusMergeable = $state<boolean | null>(null);
  let prStatusDraft = $state<boolean | null>(null);
  let failedChecks = $state<PrFailedCheck[]>([]);

  // Derive push action from the git-state upstream relation when available,
  // falling back to a HEAD/PR SHA comparison only when upstream is missing
  // (e.g. fork PRs without an `origin/<branch>` ref).
  let pushAction = $derived(
    derivePrPushAction({
      prNumber: branch.prNumber,
      prState: branch.prState,
      prHeadSha,
      gitState: timeline?.gitState ?? null,
    })
  );
  let hasUnpushed = $derived(pushAction !== 'none');

  // Sync local PR status state when branch prop changes
  let syncedBranchId = $state<string | null>(null);
  $effect(() => {
    if (branch.id !== syncedBranchId) {
      syncedBranchId = branch.id;
      failedChecks = [];
      prStatusCleared = false;
    }
    prStatusState = branch.prState;
    prStatusChecks = branch.prChecksStatus;
    prStatusReviewDecision = branch.prReviewDecision;
    prStatusMergeable = branch.prMergeable;
    prStatusDraft = branch.prDraft;
    prHeadSha = branch.prHeadSha;
    prFetchedAt = branch.prFetchedAt;
    if (branch.prChecksStatus !== 'FAILURE') {
      failedChecks = [];
    }
    if (branch.prChecksStatus) {
      prStatusCleared = false;
    }
  });

  // =========================================================================
  // Push button state — derived directly from the global pushStateStore
  // =========================================================================
  let storePushState = $derived(pushStateStore.getPushState(branch.id));
  let pushState = $derived<PushState>(storePushState?.state ?? 'idle');
  let pushSessionId = $derived(storePushState?.sessionId ?? null);
  let pushError = $derived(storePushState?.error ?? null);
  let pushRejectedNonFastForward = $derived(storePushState?.rejectedNonFastForward ?? false);
  let pushForce = $derived(storePushState?.force ?? false);
  let showPushErrorDialog = $state(false);
  let showForcePushDialog = $state(false);

  // =========================================================================
  // Event listeners for PR status
  // =========================================================================
  $effect(() => {
    const branchId = branch.id;

    const unlistenStatus = onBranchPrStatusChanged(branchId, (payload) => {
      prStatusState = payload.prState;
      prStatusChecks = payload.prChecksStatus;
      prStatusReviewDecision = payload.prReviewDecision;
      prStatusMergeable = payload.prMergeable;
      prStatusDraft = payload.prDraft;
      prHeadSha = payload.prHeadSha;
      prFetchedAt = payload.prFetchedAt;
      failedChecks = payload.failedChecks ?? [];
      prStatusCleared = false;
      // Update the polling service with the new checks status
      prPollingService.updateChecksStatus(
        branchId,
        branch.projectId,
        payload.prChecksStatus === 'PENDING'
      );
    });

    const unlistenCleared = onBranchPrStatusCleared(branchId, () => {
      prStatusState = null;
      prStatusChecks = null;
      prStatusReviewDecision = null;
      prStatusMergeable = null;
      prStatusDraft = null;
      prHeadSha = null;
      prFetchedAt = null;
      failedChecks = [];
      prStatusCleared = true;
      prPollingService.updateChecksStatus(branchId, branch.projectId, false);
    });

    return () => {
      unlistenStatus();
      unlistenCleared();
    };
  });

  // Fallback polling for PR session. A rejection means the backend was
  // unreachable (a missing session resolves to `null`), so give it a few
  // consecutive attempts before declaring the session lost.
  //
  // The `'__pending__'` sentinel is skipped: handleCreatePr seeds it before
  // `createPr` resolves, and polling it would classify a PR that is about to start
  // as gone. `prSessionId` is derived from the store, so the effect re-runs with the
  // real id the moment setPrCreating swaps it in.
  $effect(() => {
    if (prState !== 'creating' || !prSessionId || prSessionId === '__pending__') return;

    const sid = prSessionId;
    const failures = createPollFailureTracker();
    const interval = setInterval(async () => {
      try {
        const session = await commands.getSession(sid);
        failures.recordSuccess();
        switch (classifyPolledSession(session)) {
          case 'gone':
            // Deleted out from under us; revert the button to "Create PR" rather
            // than leaving it on "Creating PR…" forever.
            console.warn(
              `[BranchCardPrButton] PR session ${sid} for branch ${branch.id} no longer exists; clearing its state.`
            );
            prStateStore.clearPrState(branch.id);
            break;
          case 'waiting':
          case 'active':
            // PR sessions launch running today, but tolerating a queued one keeps
            // the poller from misreporting it as a completion.
            break;
          case 'finished':
            if (session) handlePrSessionComplete(session.status);
            break;
        }
      } catch (err) {
        if (!failures.recordFailure()) {
          console.warn(
            `[BranchCardPrButton] Could not poll PR session ${sid} for branch ${branch.id}, retrying:`,
            err
          );
          return;
        }
        prStateStore.setPrError(branch.id, 'Lost track of PR creation session.');
        prStateStore.clearSessionTracking(branch.id);
      }
    }, 5_000);

    return () => clearInterval(interval);
  });

  // Fallback polling for push session. Queued pushes are polled too: if the
  // branch queue drains one while the "running" event is missed, this is what
  // moves the button off "Queued". Transient poll failures are tolerated for a
  // few attempts, so a single blip can't flip a still-queued push to "Push
  // failed".
  //
  // The `'__pending__'` sentinel is skipped for the same reason as in the PR
  // poller above: handlePush (and BranchCard's push rows) seed it before
  // `pushBranch` resolves, and it is not an id the backend can find.
  $effect(() => {
    if (
      (pushState !== 'pushing' && pushState !== 'queued') ||
      !pushSessionId ||
      pushSessionId === '__pending__'
    )
      return;

    const sid = pushSessionId;
    const failures = createPollFailureTracker();
    const interval = setInterval(async () => {
      try {
        const session = await commands.getSession(sid);
        failures.recordSuccess();
        switch (classifyPolledSession(session)) {
          case 'gone':
            // Not setPushError: flipping to "Push failed" for a session someone
            // deliberately deleted misreports it, and clearSessionTracking would
            // keep the "Queued" badge with a Cancel button that can't work.
            console.warn(
              `[BranchCardPrButton] Push session ${sid} for branch ${branch.id} no longer exists; clearing its state.`
            );
            pushStateStore.clearPushState(branch.id);
            break;
          case 'waiting':
            break;
          case 'active':
            pushStateStore.markQueuedPushStarted(branch.id, sid);
            break;
          case 'finished':
            if (session) handlePushSessionComplete(session.status, session);
            break;
        }
      } catch (err) {
        if (!failures.recordFailure()) {
          console.warn(
            `[BranchCardPrButton] Could not poll push session ${sid} for branch ${branch.id}, retrying:`,
            err
          );
          return;
        }
        console.error(
          `[BranchCardPrButton] Lost track of push session ${sid} for branch ${branch.id}:`,
          err
        );
        pushStateStore.setPushError(branch.id, 'Lost track of push session.');
        pushStateStore.clearSessionTracking(branch.id);
      }
    }, 5_000);

    return () => clearInterval(interval);
  });

  // Subscribe to stale-data notifications from the polling service.
  // Using $effect with cleanup so the subscription is immune to double-mount
  // (e.g. HMR, keyed re-render) and automatically tracks branch.projectId.
  $effect(() => {
    const projectId = branch.projectId;
    const unsub = prPollingService.onStale((staleProjectId, isStale) => {
      if (staleProjectId === projectId) {
        prStatusStale = isStale;
      }
    });
    return () => unsub();
  });

  // Subscribe to per-project refresh-state notifications so the tooltip can
  // distinguish "last checked" from "checking right now".
  $effect(() => {
    const projectId = branch.projectId;
    prStatusRefreshing = prPollingService.isRefreshing(projectId);
    const unsub = prPollingService.onRefreshing((refreshingProjectId, isRefreshing) => {
      if (refreshingProjectId === projectId) {
        prStatusRefreshing = isRefreshing;
      }
    });
    return () => unsub();
  });

  onMount(() => {
    window.addEventListener('keydown', handleOptionDown);
    window.addEventListener('keyup', handleOptionUp);

    // PR recovery: if the branch has been pushed but has no PR number,
    // check GitHub for an open PR that passes the same checks as detection.
    // The shouldAttemptRecovery guard prevents N concurrent `gh pr list`
    // CLI calls when many components mount simultaneously.
    if (!branch.prNumber && isRemote && prPollingService.shouldAttemptRecovery(branch.id)) {
      commands
        .recoverBranchPr(branch.id)
        .then((prNumber) => {
          if (prNumber) {
            branch.prNumber = prNumber;
            prPollingService.refreshNow(branch.projectId);
          }
        })
        .catch(() => {
          // PR recovery is best-effort; clear the guard so it can be
          // retried on next mount (e.g. after a transient network error).
          prPollingService.clearRecoveryAttempt(branch.id);
        });
    }
  });

  onDestroy(() => {
    window.removeEventListener('keydown', handleOptionDown);
    window.removeEventListener('keyup', handleOptionUp);
  });

  // =========================================================================
  // PR status display
  // =========================================================================

  function getPrStatusText(): string | null {
    if (!branch.prNumber) return null;

    if (prStatusState === 'MERGED') return 'Merged';
    if (prStatusState === 'CLOSED') return 'Closed';
    if (prStatusDraft) return 'Draft';

    if (prStatusChecks === 'FAILURE') return 'Checks failing';
    if (prStatusChecks === 'PENDING') return 'Checks pending';

    if (prStatusReviewDecision === 'CHANGES_REQUESTED') return 'Changes requested';
    if (prStatusReviewDecision === 'APPROVED' && prStatusMergeable) return 'Approved';
    if (prStatusReviewDecision === 'APPROVED') return 'Approved';

    if (prStatusMergeable === false) return 'Has conflicts';
    if (prStatusChecks === 'SUCCESS') return 'Open';

    return null;
  }

  let prStatusText = $derived(getPrStatusText());

  function getPrStatusIndicator(): 'success' | 'warning' | 'error' | 'neutral' | 'pending' | null {
    if (prState === 'creating') return null;
    if (pushState === 'pushing' || pushState === 'queued') return null;
    if (pushState === 'error' || prState === 'error') return 'error';

    if (!branch.prNumber) return null;

    if (prState === 'created' && hasUnpushed && pushState === 'idle') return null;

    if (prStatusState === 'MERGED') return null;
    if (prStatusState === 'CLOSED') return 'neutral';
    if (prStatusDraft) return 'neutral';

    if (prStatusMergeable === false) return 'error';

    if (prStatusChecks === 'FAILURE') return 'error';
    if (prStatusChecks === 'PENDING') return 'pending';
    if (prStatusChecks === 'SUCCESS') return 'success';

    if (prStatusReviewDecision === 'CHANGES_REQUESTED') return 'warning';
    if (prStatusReviewDecision === 'APPROVED') return 'success';

    return 'neutral';
  }

  let prStatusIndicator = $derived(getPrStatusIndicator());

  function getPrButtonActionTitle(): string {
    if (pushState === 'queued') return 'Push queued behind branch work — click to cancel';
    if (pushState === 'pushing') return 'Pushing… (click to view)';
    if (pushState === 'error') return 'Push failed — click for details';
    if (prState === 'created' && hasUnpushed) {
      return pushAction === 'forcePush' || optionHeld
        ? 'Force push to remote'
        : 'Push changes to remote';
    }
    if (prState === 'created') {
      if (prStatusState === 'MERGED') return 'Merged';
      if (prStatusState === 'CLOSED') return 'Closed';
      if (prStatusDraft) return 'Draft';
      if (prStatusChecks === 'FAILURE') return 'Checks failing';
      if (prStatusChecks === 'PENDING') return 'Checks pending';
      if (prStatusReviewDecision === 'CHANGES_REQUESTED') return 'Changes requested';
      if (prStatusMergeable === false) return 'Has conflicts';
      return `View PR${branch.prNumber ? ` #${branch.prNumber}` : ''}`;
    }
    if (prState === 'error') return 'PR creation failed — click for details';
    if (prState === 'creating') return 'Creating PR… (click to view)';
    return optionHeld ? 'Create draft PR' : 'Create PR';
  }

  let prButtonTitleNowMs = $derived.by(() => {
    if (!prFetchedAt) return undefined;

    const nowMs = minuteNow.now();
    return nowMs - prFetchedAt < 60_000 ? secondNow.now() : nowMs;
  });

  let prButtonTitle = $derived(
    buildPrButtonTitle({
      actionTitle: getPrButtonActionTitle(),
      prNumber: branch.prNumber,
      prHeadSha,
      prFetchedAt,
      checksStatus: prStatusChecks,
      statusStale: prStatusStale,
      statusRefreshing: prStatusRefreshing,
      hasUnpushed: prState === 'created' && hasUnpushed && pushState === 'idle',
      failedChecks,
      statusCleared: prStatusCleared,
      nowMs: prButtonTitleNowMs,
    })
  );

  // =========================================================================
  // PR creation
  // =========================================================================

  function handleCreatePr(draft = false) {
    if (prState === 'creating') return;

    prStateStore.setPrCreating(branch.id, '__pending__');

    const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
    const provider = getPreferredAgent(agents) ?? undefined;

    commands
      .createPr(branch.id, provider, draft)
      .then((sessionId) => {
        // Session is already registered by the global listener via the
        // backend's "running" event — just update the local store with the
        // real session ID so the fallback poller can track it.
        prStateStore.setPrCreating(branch.id, sessionId);
      })
      .catch((e) => {
        prStateStore.setPrError(branch.id, e instanceof Error ? e.message : String(e));
      });
  }

  // =========================================================================
  // Detect existing PR
  // =========================================================================
  //
  // Staged only learns a PR number when it drove the creation, so a PR opened
  // from a session's `gh pr create`, the browser or a terminal leaves the
  // button on "Create PR". This asks the backend to look the branch up on
  // GitHub; the backend persists a match and reports near-misses with reasons.
  //
  // A match only updates the button (the PR badge is the confirmation); every
  // other outcome opens an alert dialog so the user can tell "no PR" apart from
  // "a PR exists but targets develop" or "gh failed".
  //
  // A candidate rejected on its base branch alone (the backend marks it
  // `linkable`) gets a "Link anyway" action: the stored base can be stale after
  // a stack restructure, or the PR may have been retargeted on GitHub.
  type PrDetectionFailure =
    | { kind: 'notFound' }
    | { kind: 'rejected'; candidates: RejectedPrCandidate[] }
    | { kind: 'error'; message: string };

  let detectingPr = $state(false);
  let prDetectionFailure = $state<PrDetectionFailure | null>(null);
  let showPrDetectionDialog = $derived(prDetectionFailure !== null);
  let linkingPrNumber = $state<number | null>(null);
  let prLinkError = $state<string | null>(null);

  // The secondary menu only joins the button while it still reads "Create PR".
  let showCreatePrMenu = $derived(prState === 'idle' && pushState === 'idle');

  // Mirror what the backend persisted, as handlePrSessionComplete does.
  function adoptLinkedPr(prNumber: number, prUrl: string) {
    branch.prNumber = prNumber;
    prStateStore.setPrCreated(branch.id, prUrl);
    prPollingService.refreshNow(branch.projectId);
  }

  async function handleDetectExistingPr() {
    if (detectingPr || prState !== 'idle') return;

    detectingPr = true;
    try {
      const result = await commands.detectBranchPr(branch.id);
      switch (result.kind) {
        case 'found':
          adoptLinkedPr(result.prNumber, result.prUrl);
          break;
        case 'notFound':
          prDetectionFailure = { kind: 'notFound' };
          break;
        case 'rejected':
          prDetectionFailure = { kind: 'rejected', candidates: result.candidates };
          break;
      }
    } catch (e) {
      prDetectionFailure = { kind: 'error', message: e instanceof Error ? e.message : String(e) };
    } finally {
      detectingPr = false;
    }
  }

  // The failure is shown inside the open dialog rather than replacing it, so
  // the candidate list stays visible next to the explanation.
  async function handleLinkPrAnyway(candidate: RejectedPrCandidate) {
    if (linkingPrNumber !== null) return;

    linkingPrNumber = candidate.prNumber;
    prLinkError = null;
    try {
      const linked = await commands.linkBranchPr(branch.id, candidate.prNumber);
      adoptLinkedPr(linked.prNumber, linked.prUrl);
      prDetectionFailure = null;
    } catch (e) {
      prLinkError = e instanceof Error ? e.message : String(e);
    } finally {
      linkingPrNumber = null;
    }
  }

  function handlePrDetectionDialogClose() {
    prDetectionFailure = null;
    prLinkError = null;
  }

  let prCompletionInFlight = false;

  // Renderer-only fallback for completions observed outside the event stream
  // (e.g. found finished on session-modal close). The backend parses and
  // persists the PR number at the terminal transition and emits `pr-created`;
  // this must not write, only mirror the outcome locally.
  export async function handlePrSessionComplete(status: string) {
    if (prCompletionInFlight) return;
    const sid = prSessionId;
    prCompletionInFlight = true;
    prStateStore.clearSessionTracking(branch.id);

    try {
      if (status === 'completed' && sid) {
        const messages = await commands.getFreshSessionMessages(sid);
        const foundUrl = extractPrUrl(messages);

        if (foundUrl) {
          const prNumber = extractPrNumber(foundUrl);
          if (prNumber) {
            branch.prNumber = prNumber;
            prPollingService.refreshNow(branch.projectId);
          }
          prStateStore.setPrCreated(branch.id, foundUrl);
        } else {
          prStateStore.setPrError(
            branch.id,
            'PR session completed but no PR URL was found in the output.'
          );
        }
      } else {
        prStateStore.setPrError(
          branch.id,
          `PR creation session ${status === 'error' ? 'failed' : 'was cancelled'}.`
        );
      }
    } catch (e) {
      prStateStore.setPrError(branch.id, e instanceof Error ? e.message : String(e));
    } finally {
      prCompletionInFlight = false;
    }
  }

  // =========================================================================
  // Push
  // =========================================================================

  function handlePush(force = false) {
    if (pushState === 'pushing' || pushState === 'queued') return;

    pushStateStore.setPushing(branch.id, '__pending__', force);

    const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
    const provider = getPreferredAgent(agents) ?? undefined;

    commands
      .pushBranch(branch.id, provider, force)
      .then((response) => {
        // A running session is already registered by the global listener via
        // the backend's "running" event — this just records the real session ID
        // (and whether the push is waiting on the branch queue) so the fallback
        // poller can track it.
        pushStateStore.setPushLaunch(branch.id, response, force);
      })
      .catch((e) => {
        pushStateStore.setPushError(branch.id, e instanceof Error ? e.message : String(e));
      });
  }

  /**
   * Drop a push that is still waiting on the branch queue.
   *
   * The store entry is cleared here rather than in the completion handler: a
   * session that never ran isn't in the session registry, so the cancellation
   * event carries no push session type for `sessionStatusListener` to match. It
   * waits for the backend to confirm the cancellation — see
   * `createQueuedSessionCanceller`.
   *
   * A failure is toasted rather than pushed into `setPushError`: the push is still
   * queued, so flipping this button to "Push failed" would both misreport its state
   * and swap the Cancel affordance for a retry dialog.
   */
  const runCancelQueuedPush = createQueuedSessionCanceller({
    cancel: (sessionId) => commands.cancelSession(sessionId),
    clearState: () => pushStateStore.clearPushState(branch.id),
    onError: (e) =>
      toast.error('Could not cancel queued push', {
        description: e instanceof Error ? e.message : String(e),
        duration: Infinity,
      }),
  });

  function cancelQueuedPush() {
    const sid = pushSessionId;
    if (pushState !== 'queued' || !sid || sid === '__pending__') return;

    void runCancelQueuedPush(sid);
  }

  let pushCompletionInFlight = false;

  async function classifyPushSessionOutcome(
    sid: string,
    completedSession?: Session | null
  ): Promise<CompletedPushOutcome> {
    let pipeline = completedSession?.pipeline ?? null;

    if (!completedSession) {
      try {
        const session = await commands.getSession(sid);
        pipeline = session?.pipeline ?? null;
      } catch {
        // Fall back to message markers below.
      }
    }

    try {
      const messages = await commands.getFreshSessionMessages(sid);
      const pipelineOutcome = classifyPipelinePushCompletion(pipeline, messages);
      if (pipelineOutcome) return pipelineOutcome;
      return classifyCompletedPushSession(pipeline, messages);
    } catch {
      // If messages can't be fetched, try pipeline-only classification (without
      // the force-push false-positive guard) then fall back to succeeded.
      const pipelineOutcome = classifyPipelinePushCompletion(pipeline);
      if (pipelineOutcome) return pipelineOutcome;
      return 'succeeded';
    }
  }

  // Renderer-only fallback, like handlePrSessionComplete above: the backend
  // classifies the push at the terminal transition, clears the stale PR
  // status, and emits `push-completed`; this only mirrors the outcome and
  // refreshes read-side caches.
  export async function handlePushSessionComplete(status: string, completedSession?: Session) {
    if (pushCompletionInFlight) return;
    const sid = pushSessionId;
    pushCompletionInFlight = true;
    pushStateStore.clearSessionTracking(branch.id);

    try {
      if (status === 'completed' && sid) {
        const outcome = await classifyPushSessionOutcome(sid, completedSession);

        if (outcome === 'rejected_non_fast_forward') {
          pushStateStore.setPushError(branch.id, '', true);
        } else {
          pushStateStore.setPushDone(branch.id);
          // Refresh local git state so `upstream.relation` settles back to
          // `inSync` (driving hasUnpushed to false) without optimistically
          // mutating prHeadSha here. Force-bypass the TTL: a recent
          // window-focus or timeline-mount refresh can otherwise short-circuit
          // the `git fetch` and leave `refs/remotes/origin/<branch>` stale.
          commands.refreshBranchGitState(branch.id, { force: true }).catch((e) => {
            console.warn('[Staged] Failed to refresh git state after push:', e);
          });
          // Immediately refresh PR status so checks update right away
          prPollingService.refreshNow(branch.projectId);
          setTimeout(() => {
            pushStateStore.clearPushState(branch.id);
          }, 1_500);
        }
      } else {
        pushStateStore.setPushError(
          branch.id,
          `Push session ${status === 'error' ? 'failed' : 'was cancelled'}.`
        );
      }
    } finally {
      pushCompletionInFlight = false;
    }
  }

  function handleForcePushConfirm() {
    showForcePushDialog = false;
    handlePush(true);
  }

  function handleForcePushCancel() {
    showForcePushDialog = false;
    pushStateStore.clearPushState(branch.id);
  }

  // Retry repeats the failed push as it was asked for: a force push that
  // errored must not come back as a plain push the diverged remote rejects.
  function handlePushErrorRetry() {
    showPushErrorDialog = false;
    handlePush(pushForce);
  }

  function handlePushErrorClose() {
    showPushErrorDialog = false;
    pushStateStore.clearPushState(branch.id);
  }

  function handlePrButtonClick() {
    if (pushState === 'error') {
      if (pushRejectedNonFastForward) {
        showForcePushDialog = true;
      } else {
        showPushErrorDialog = true;
      }
      return;
    }
    if (pushState === 'queued') {
      cancelQueuedPush();
      return;
    }
    if (pushState === 'pushing' && pushSessionId) {
      onOpenSession?.(pushSessionId);
      return;
    }
    if (prState === 'created' && hasUnpushed && pushState === 'idle') {
      const wantsForcePush = pushAction === 'forcePush' || optionHeld;
      if (wantsForcePush && !optionHeld) {
        // Force push without an explicit alt override — confirm first.
        showForcePushDialog = true;
      } else {
        handlePush(wantsForcePush);
      }
    } else if (prState === 'created') {
      const url = prUrl ?? cachedPrUrl;
      if (url) {
        commands.openUrl(url).catch((e) => console.error('Failed to open PR URL:', e));
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
      showPrErrorDialog = true;
    } else if (prState === 'idle') {
      handleCreatePr(optionHeld);
    } else if (prState === 'creating' && prSessionId) {
      onOpenSession?.(prSessionId);
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

  // Expose session IDs and state for parent coordination
  export function getPrSessionId(): string | null {
    return prSessionId;
  }

  export function getPushSessionId(): string | null {
    return pushSessionId;
  }

  export function getPrCreatingState(): PrState {
    return prState;
  }

  export function getPushingState(): PushState {
    return pushState;
  }
</script>

{#snippet prButton(props: Record<string, unknown>)}
  <span {...props} class="inline-flex min-w-0 max-w-full">
    <Button
      variant="outline"
      size="sm"
      class={[
        'min-w-0 max-w-full !shrink gap-1.5 whitespace-nowrap text-xs font-medium [&_svg]:!size-3.5',
        prState === 'creating' && 'border-[var(--border-muted)]',
        (prState === 'error' || pushState === 'error') &&
          'border-destructive text-destructive hover:bg-[var(--ui-danger-bg)] hover:text-destructive',
        pushState === 'pushing' && 'cursor-default border-[var(--border-muted)]',
        pushState === 'queued' && 'border-[var(--border-muted)]',
        prState === 'created' && prStatusState === 'MERGED' && '[&_svg]:text-[var(--status-added)]',
        showCreatePrMenu && 'rounded-r-none',
        detectingPr && 'cursor-default border-[var(--border-muted)]',
      ]}
      onclick={handlePrButtonClick}
      disabled={showPushErrorDialog ||
        showForcePushDialog ||
        showPrErrorDialog ||
        showPrDetectionDialog ||
        detectingPr}
    >
      {#if pushState === 'queued'}
        <Clock size={13} />
      {:else if pushState === 'pushing'}
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
      {:else if detectingPr}
        <Spinner size={13} />
      {:else}
        <GitPullRequestCreateArrow size={13} />
      {/if}
      <span class="min-w-0 truncate">
        {#if pushState === 'queued'}
          Push queued
        {:else if pushState === 'pushing'}
          Pushing…
        {:else if pushState === 'error'}
          Push failed
        {:else if prState === 'created' && hasUnpushed}
          {pushAction === 'forcePush' || optionHeld ? 'Force push' : 'Push changes'}
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
        {:else if detectingPr}
          Detecting PR…
        {:else}
          {optionHeld ? 'Create draft PR' : 'Create PR'}
        {/if}
      </span>
      {#if prStatusStale}
        <span class="pr-status-stale" title="PR status may be outdated">!</span>
      {:else if prStatusIndicator}
        <span class="pr-status-indicator {prStatusIndicator}"></span>
      {/if}
    </Button>
    {#if showCreatePrMenu}
      <!-- Joined to the Create PR button, as the rebase chevron is in
           BranchCardHeaderInfo: same outline and height, touching corners
           squared, border overlapped so the pair reads as one control.
           The tooltip lives on a wrapping span, as it does there, because
           browsers don't reliably show tooltips on a disabled element. -->
      <DropdownMenu.Root>
        <span class="inline-flex -ml-px shrink-0" title="More PR options">
          <DropdownMenu.Trigger
            class={cn(
              buttonVariants({ variant: 'outline', size: 'sm' }),
              'w-6 rounded-l-none px-0 [&_svg]:!size-3.5',
              detectingPr && 'border-[var(--border-muted)]'
            )}
            disabled={showPrDetectionDialog || detectingPr}
            aria-label="More PR options"
          >
            <ChevronDown size={13} />
          </DropdownMenu.Trigger>
        </span>
        <DropdownMenu.Content align="end" sideOffset={4} class="min-w-[200px]">
          <DropdownMenu.Item onSelect={() => handleCreatePr(true)}>
            <GitPullRequestDraft size={14} /> Create draft PR
          </DropdownMenu.Item>
          <DropdownMenu.Item
            title="Look up GitHub for a pull request already opened from this branch and link it here."
            onSelect={handleDetectExistingPr}
          >
            <Search size={14} /> Detect existing PR
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Root>
    {/if}
  </span>
{/snippet}

{#snippet renderButton()}
  {#if hasCodeChanges || branch.prNumber}
    {#if branch.prNumber}
      {@render prButton({ title: prButtonTitle })}
    {:else}
      {@render prButton({})}
    {/if}
  {/if}
{/snippet}

{#if showButton}
  {@render renderButton()}
{/if}

{#if showPrErrorDialog}
  <AlertDialog.Root bind:open={showPrErrorDialog}>
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title>PR Creation Failed</AlertDialog.Title>
        <AlertDialog.Description class="max-h-[42vh] overflow-auto whitespace-pre-line">
          {prError ?? 'An unknown error occurred while creating the PR.'}
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel onclick={handlePrErrorClose}>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action variant="outline" onclick={handlePrErrorRetry}>
          Retry
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>
{/if}

{#if prDetectionFailure}
  <AlertDialog.Root
    open={showPrDetectionDialog}
    onOpenChange={(open) => !open && linkingPrNumber === null && handlePrDetectionDialogClose()}
  >
    <AlertDialog.Content>
      {#if prDetectionFailure.kind === 'notFound'}
        <AlertDialog.Header>
          <AlertDialog.Title>No pull request found</AlertDialog.Title>
          <AlertDialog.Description>
            GitHub has no pull request with {branch.branchName} as its head branch.
          </AlertDialog.Description>
        </AlertDialog.Header>
      {:else if prDetectionFailure.kind === 'error'}
        <AlertDialog.Header>
          <AlertDialog.Title>Could not check GitHub for an existing PR</AlertDialog.Title>
          <AlertDialog.Description class="max-h-[42vh] overflow-auto whitespace-pre-line">
            {prDetectionFailure.message}
          </AlertDialog.Description>
        </AlertDialog.Header>
      {:else}
        {@const candidates = prDetectionFailure.candidates}
        {@const anyLinkable = candidates.some((c) => c.linkable)}
        <AlertDialog.Header>
          <AlertDialog.Title>Existing PR not linked</AlertDialog.Title>
          <AlertDialog.Description>
            GitHub has {candidates.length === 1
              ? 'a pull request'
              : `${candidates.length} pull requests`} with {branch.branchName} as the head branch, but
            none can be linked to this branch:
          </AlertDialog.Description>
        </AlertDialog.Header>
        <ul class="max-h-[42vh] space-y-3 overflow-auto text-sm">
          {#each candidates as candidate (candidate.prNumber)}
            <li>
              <div class="flex items-center justify-between gap-3">
                <div class="flex min-w-0 items-center gap-2">
                  <button
                    type="button"
                    class="font-medium underline-offset-2 hover:underline"
                    onclick={() =>
                      commands.openUrl(candidate.prUrl).catch((e) => {
                        console.error('Failed to open PR URL:', e);
                      })}
                  >
                    #{candidate.prNumber}
                  </button>
                  <span class="truncate text-[var(--text-muted)]">by {candidate.author}</span>
                  {#if candidate.draft}
                    <Badge variant="outline" title="Draft pull request">
                      <GitPullRequestDraft /> Draft
                    </Badge>
                  {/if}
                </div>
                {#if candidate.linkable}
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-7 gap-1.5 text-xs"
                    title="Link PR #{candidate.prNumber} to this branch even though it targets a different base."
                    disabled={linkingPrNumber !== null}
                    onclick={() => handleLinkPrAnyway(candidate)}
                  >
                    {#if linkingPrNumber === candidate.prNumber}
                      <Spinner size={12} /> Linking…
                    {:else}
                      Link anyway
                    {/if}
                  </Button>
                {/if}
              </div>
              <ul class="mt-1 list-disc space-y-0.5 pl-5 text-[var(--text-muted)]">
                {#each candidate.reasons as reason, i (i)}
                  <li>{describePrRejectionReason(reason)}</li>
                {/each}
              </ul>
            </li>
          {/each}
        </ul>
        {#if anyLinkable}
          <p class="text-xs text-[var(--text-muted)]">
            A base mismatch can be overridden if the PR was retargeted on GitHub or this branch's
            base is out of date. Closed, merged and fork pull requests cannot be linked.
          </p>
        {/if}
        {#if prLinkError}
          <p class="whitespace-pre-line text-sm text-destructive">{prLinkError}</p>
        {/if}
      {/if}
      <AlertDialog.Footer>
        <AlertDialog.Cancel
          disabled={linkingPrNumber !== null}
          onclick={handlePrDetectionDialogClose}
        >
          Close
        </AlertDialog.Cancel>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>
{/if}

{#if showPushErrorDialog}
  <AlertDialog.Root bind:open={showPushErrorDialog}>
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title>{pushForce ? 'Force Push Failed' : 'Push Failed'}</AlertDialog.Title>
        <AlertDialog.Description class="max-h-[42vh] overflow-auto whitespace-pre-line">
          {pushError ?? 'An unknown error occurred while pushing.'}
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel onclick={handlePushErrorClose}>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action variant="outline" onclick={handlePushErrorRetry}>
          Retry
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>
{/if}

{#if showForcePushDialog}
  <AlertDialog.Root bind:open={showForcePushDialog}>
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title>Push Rejected</AlertDialog.Title>
        <AlertDialog.Description>
          The remote branch has commits that would be lost. Do you want to force push? This will
          overwrite the remote branch with your local version.
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel onclick={handleForcePushCancel}>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action variant="destructive" onclick={handleForcePushConfirm}>
          Force Push
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>
{/if}

<style>
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

  .pr-status-stale {
    font-size: 9px;
    font-weight: 700;
    color: var(--text-faint, #94a3b8);
    margin-left: 2px;
    opacity: 0.7;
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
