<!--
  BranchCardPrButton.svelte - PR creation/push/status button

  Handles PR creation, push operations, status polling, and associated
  error/confirmation dialogs.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import {
    GitPullRequestCreateArrow,
    GitPullRequestArrow,
    GitPullRequestDraft,
    GitMerge,
    AlertCircle,
  } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import { minuteNow, secondNow } from '../../shared/relativeTime.svelte';
  import { listen } from '@tauri-apps/api/event';
  import type {
    Branch,
    BranchTimeline as BranchTimelineData,
    PrFailedCheck,
    PrStatusChangedEvent,
    Session,
  } from '../../types';
  import * as commands from '../../api/commands';
  import {
    classifyCompletedPushSession,
    classifyPipelinePushCompletion,
    extractPrNumber,
    extractPrUrl,
    type CompletedPushOutcome,
  } from './branchCardHelpers';
  import { buildPrButtonTitle } from './prButtonTooltip';
  import { getPreferredAgent } from '../settings/preferences.svelte';
  import { agentState, REMOTE_AGENTS } from '../agents/agent.svelte';
  import { prStateStore, type PrState } from '../../stores/prState.svelte';
  import { pushStateStore, type PushState } from '../../stores/pushState.svelte';
  import * as prPollingService from '../../services/prPollingService';

  interface Props {
    branch: Branch;
    isLocal: boolean;
    isRemote: boolean;
    hasCodeChanges: boolean;
    timeline: BranchTimelineData | null;
    onOpenSession?: (sessionId: string) => void;
  }

  let { branch, isLocal, isRemote, hasCodeChanges, timeline, onOpenSession }: Props = $props();

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

  // Derive hasUnpushed by comparing the latest timeline commit SHA with the PR head SHA.
  // This replaces the old approach of shelling out to `git rev-list`.
  let hasUnpushed = $derived.by(() => {
    if (!branch.prNumber || !timeline) return false;
    // Don't show push changes on merged PRs
    if (branch.prState === 'MERGED') return false;
    // Find the first commit with a real (non-empty) SHA — pending commits have sha: ""
    const latestCommit = timeline.commits.find((c) => c.sha && c.sha.length > 0);
    if (!latestCommit || !prHeadSha) return false;
    return latestCommit.sha !== prHeadSha;
  });

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
  let showPushErrorDialog = $state(false);
  let showForcePushDialog = $state(false);

  // =========================================================================
  // Event listeners for PR status (fix race condition by awaiting promises)
  // =========================================================================
  $effect(() => {
    const branchId = branch.id;

    const unlistenStatusPromise = listen<PrStatusChangedEvent>('pr-status-changed', (event) => {
      const payload = event.payload;
      if (payload.branchId === branchId) {
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
      }
    });

    const unlistenClearedPromise = listen<string>('pr-status-cleared', (event) => {
      if (event.payload === branchId) {
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
      }
    });

    return () => {
      unlistenStatusPromise.then((fn) => fn());
      unlistenClearedPromise.then((fn) => fn());
    };
  });

  // Fallback polling for PR session
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
        prStateStore.setPrError(branch.id, 'Lost track of PR creation session.');
        prStateStore.clearSessionTracking(branch.id);
      }
    }, 5_000);

    return () => clearInterval(interval);
  });

  // Fallback polling for push session
  $effect(() => {
    if (pushState !== 'pushing' || !pushSessionId) return;

    const sid = pushSessionId;
    const interval = setInterval(async () => {
      try {
        const session = await commands.getSession(sid);
        if (session && session.status !== 'running') {
          handlePushSessionComplete(session.status, session);
        }
      } catch (err) {
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
    // check GitHub for an existing open PR on this branch name.
    // The shouldAttemptRecovery guard prevents N concurrent `gh pr view`
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
    if (pushState === 'pushing') return null;
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
    if (pushState === 'pushing') return 'Pushing… (click to view)';
    if (pushState === 'error') return 'Push failed — click for details';
    if (prState === 'created' && hasUnpushed) {
      return optionHeld ? 'Force push to remote' : 'Push changes to remote';
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

  let prCompletionInFlight = false;

  export async function handlePrSessionComplete(status: string) {
    if (prCompletionInFlight) return;
    const sid = prSessionId;
    prCompletionInFlight = true;
    prStateStore.clearSessionTracking(branch.id);

    try {
      if (status === 'completed' && sid) {
        const messages = await commands.getSessionMessages(sid);
        const foundUrl = extractPrUrl(messages);

        if (foundUrl) {
          const prNumber = extractPrNumber(foundUrl);
          if (prNumber) {
            await commands.updateBranchPr(branch.id, prNumber);
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
    if (pushState === 'pushing') return;

    pushStateStore.setPushing(branch.id, '__pending__');

    const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
    const provider = getPreferredAgent(agents) ?? undefined;

    commands
      .pushBranch(branch.id, provider, force)
      .then((sessionId) => {
        // Session is already registered by the global listener via the
        // backend's "running" event — just update the local store with the
        // real session ID so the fallback poller can track it.
        pushStateStore.setPushing(branch.id, sessionId);
      })
      .catch((e) => {
        pushStateStore.setPushError(branch.id, e instanceof Error ? e.message : String(e));
      });
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
      const messages = await commands.getSessionMessages(sid);
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
          try {
            await commands.clearBranchPrStatus(branch.id);
          } catch (e) {
            console.warn('[Staged] Failed to clear PR status after push:', e);
          }
          pushStateStore.setPushDone(branch.id);
          // Optimistically update prHeadSha to the latest timeline commit
          // so hasUnpushed becomes false immediately, before the next PR
          // status refresh picks up the new head SHA from GitHub.
          const latestCommit = timeline?.commits.find((c) => c.sha && c.sha.length > 0);
          if (latestCommit) {
            prHeadSha = latestCommit.sha;
          }
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
      if (pushRejectedNonFastForward) {
        showForcePushDialog = true;
      } else {
        showPushErrorDialog = true;
      }
      return;
    }
    if (pushState === 'pushing' && pushSessionId) {
      onOpenSession?.(pushSessionId);
      return;
    }
    if (prState === 'created' && hasUnpushed && pushState === 'idle') {
      handlePush(optionHeld);
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

{#if hasCodeChanges}
  <button
    class="pr-btn"
    class:creating={prState === 'creating'}
    class:error={prState === 'error' || pushState === 'error'}
    class:created={prState === 'created' && pushState !== 'error'}
    class:pushing={pushState === 'pushing'}
    class:merged={prState === 'created' && prStatusState === 'MERGED'}
    onclick={handlePrButtonClick}
    disabled={showPushErrorDialog || showForcePushDialog || showPrErrorDialog}
    title={prButtonTitle}
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
        {optionHeld ? 'Force push' : 'Push changes'}
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
    {#if prStatusStale}
      <span class="pr-status-stale" title="PR status may be outdated">!</span>
    {:else if prStatusIndicator}
      <span class="pr-status-indicator {prStatusIndicator}"></span>
    {/if}
  </button>
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
