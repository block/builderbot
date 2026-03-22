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
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import type { Branch, BranchTimeline as BranchTimelineData } from '../../types';
  import * as commands from '../../api/commands';
  import { extractPrNumber, extractPrUrl, isPushRejectedNonFastForward } from './branchCardHelpers';
  import { getPreferredAgent } from '../settings/preferences.svelte';
  import { agentState, REMOTE_AGENTS } from '../agents/agent.svelte';
  import { prStateStore, type PrState } from '../../stores/prState.svelte';
  import { pushStateStore, type PushState } from '../../stores/pushState.svelte';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import { sessionRegistry } from '../../stores/sessionRegistry.svelte';

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

  // Unpushed-commits state (only relevant when PR already exists)
  let hasUnpushed = $state(false);

  // PR status polling state
  let prStatusPollTimer: ReturnType<typeof setInterval> | null = null;
  let prStatusRefreshing = $state(false);

  // PR status fields (local state, updated via events)
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
  // =========================================================================
  let storePushState = $derived(pushStateStore.getPushState(branch.id));
  let pushState = $derived<PushState>(storePushState?.state ?? 'idle');
  let pushSessionId = $derived(storePushState?.sessionId ?? null);
  let pushError = $derived(storePushState?.error ?? null);
  let pushRejectedNonFastForward = $derived(storePushState?.rejectedNonFastForward ?? false);
  let showPushErrorDialog = $state(false);
  let showForcePushDialog = $state(false);

  // Window focus tracking for smart polling
  let isWindowFocused = $state(true);
  let handleFocus: (() => void) | null = null;
  let handleBlur: (() => void) | null = null;

  let unlistenPrStatus: UnlistenFn | null = null;
  let unlistenPrStatusCleared: UnlistenFn | null = null;

  $effect(() => {
    const branchId = branch.id;
    console.info(`[BranchCardPrButton] Setting up event listeners for branch=${branchId}`);

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
        console.info(
          `[BranchCardPrButton] pr-status-changed received for branch=${branchId}: state=${payload.prState}, checks=${payload.prChecksStatus}, mergeable=${payload.prMergeable}, draft=${payload.prDraft}`
        );
        prStatusState = payload.prState;
        prStatusChecks = payload.prChecksStatus;
        prStatusReviewDecision = payload.prReviewDecision;
        prStatusMergeable = payload.prMergeable;
        prStatusDraft = payload.prDraft;
      }
    }).then((unlisten) => {
      unlistenPrStatus = unlisten;
    });

    listen<string>('pr-status-cleared', (event) => {
      if (event.payload === branchId) {
        console.info(`[BranchCardPrButton] pr-status-cleared received for branch=${branchId}`);
        prStatusState = null;
        prStatusChecks = null;
        prStatusReviewDecision = null;
        prStatusMergeable = null;
        prStatusDraft = null;
      }
    }).then((unlisten) => {
      unlistenPrStatusCleared = unlisten;
    });

    return () => {
      console.info(`[BranchCardPrButton] Tearing down event listeners for branch=${branchId}`);
      unlistenPrStatus?.();
      unlistenPrStatusCleared?.();
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
          handlePushSessionComplete(session.status);
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

  // Re-check unpushed commits whenever the timeline refreshes and a PR exists.
  // For remote branches the backend uses spawn_blocking so this won't freeze
  // the UI — the button just shows "View PR" until the check completes.
  $effect(() => {
    if (timeline && branch.prNumber) {
      console.info(`[BranchCardPrButton] Checking hasUnpushedCommits for branch=${branch.id}`);
      commands
        .hasUnpushedCommits(branch.id)
        .then((v) => {
          console.info(
            `[BranchCardPrButton] hasUnpushedCommits result for branch=${branch.id}: ${v}`
          );
          hasUnpushed = v;
        })
        .catch((e) => {
          console.error(
            `[BranchCardPrButton] hasUnpushedCommits failed for branch=${branch.id}:`,
            e
          );
        });
    }
  });

  // PR status polling: adaptive intervals based on status
  $effect(() => {
    const shouldPoll = branch.prNumber && isWindowFocused;

    if (prStatusState === 'MERGED' || prStatusState === 'CLOSED') {
      console.info(
        `[BranchCardPrButton] Polling stopped for branch=${branch.id}: status=${prStatusState}`
      );
      if (prStatusPollTimer) {
        clearInterval(prStatusPollTimer);
        prStatusPollTimer = null;
      }
      return;
    }

    let pollInterval: number;
    if (prStatusChecks === 'PENDING') {
      pollInterval = 15_000;
    } else {
      pollInterval = 60_000;
    }

    if (shouldPoll) {
      console.info(
        `[BranchCardPrButton] Polling started for branch=${branch.id}, interval=${pollInterval}ms, prNumber=${branch.prNumber}, focused=${isWindowFocused}`
      );
      if (prStatusPollTimer) {
        clearInterval(prStatusPollTimer);
      }

      prStatusPollTimer = setInterval(async () => {
        if (prStatusRefreshing) {
          console.info(
            `[BranchCardPrButton] Poll tick skipped for branch=${branch.id}: refresh already in progress`
          );
          return;
        }
        try {
          console.info(`[BranchCardPrButton] Poll tick firing for branch=${branch.id}`);
          prStatusRefreshing = true;
          await commands.refreshPrStatus(branch.id);
          console.info(`[BranchCardPrButton] Poll refresh completed for branch=${branch.id}`);
        } catch (e) {
          console.error(`[BranchCardPrButton] Poll refresh failed for branch=${branch.id}:`, e);
        } finally {
          prStatusRefreshing = false;
        }
      }, pollInterval);
    } else {
      console.info(
        `[BranchCardPrButton] Polling stopped for branch=${branch.id}: prNumber=${branch.prNumber}, focused=${isWindowFocused}`
      );
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
    console.info(
      `[BranchCardPrButton] Mounted: branch=${branch.id}, hasCodeChanges=${hasCodeChanges}, prNumber=${branch.prNumber}`
    );
    window.addEventListener('keydown', handleOptionDown);
    window.addEventListener('keyup', handleOptionUp);

    handleFocus = () => {
      isWindowFocused = true;
      console.info(
        `[BranchCardPrButton] Window focused: branch=${branch.id}, prNumber=${branch.prNumber}, refreshing=${prStatusRefreshing}`
      );
      if (branch.prNumber && !prStatusRefreshing) {
        commands
          .refreshPrStatus(branch.id)
          .catch((e) => console.error('Failed to refresh PR status on focus:', e));
      }
    };
    handleBlur = () => {
      console.info(`[BranchCardPrButton] Window blurred: branch=${branch.id}`);
      isWindowFocused = false;
    };
    window.addEventListener('focus', handleFocus);
    window.addEventListener('blur', handleBlur);

    if (branch.prNumber) {
      commands
        .refreshPrStatus(branch.id)
        .catch((e) => console.error('Failed to fetch initial PR status:', e));
    }
  });

  onDestroy(() => {
    unlistenPrStatus?.();
    unlistenPrStatusCleared?.();
    if (prStatusPollTimer) {
      clearInterval(prStatusPollTimer);
      prStatusPollTimer = null;
    }
    if (handleFocus) window.removeEventListener('focus', handleFocus);
    if (handleBlur) window.removeEventListener('blur', handleBlur);
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
        sessionRegistry.register(sessionId, branch.projectId, 'pr', branch.id);
        prStateStore.setPrCreating(branch.id, sessionId);
        projectStateStore.addRunningSession(branch.projectId, sessionId);
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
            commands
              .refreshPrStatus(branch.id)
              .catch((e) => console.error('Failed to fetch initial PR status:', e));
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
        sessionRegistry.register(sessionId, branch.projectId, 'push', branch.id);
        pushStateStore.setPushing(branch.id, sessionId);
        projectStateStore.addRunningSession(branch.projectId, sessionId);
      })
      .catch((e) => {
        pushStateStore.setPushError(branch.id, e instanceof Error ? e.message : String(e));
      });
  }

  let pushCompletionInFlight = false;

  export async function handlePushSessionComplete(status: string) {
    if (pushCompletionInFlight) return;
    const sid = pushSessionId;
    pushCompletionInFlight = true;
    pushStateStore.clearSessionTracking(branch.id);

    try {
      if (status === 'completed' && sid) {
        let rejected = false;
        try {
          const messages = await commands.getSessionMessages(sid);
          rejected = isPushRejectedNonFastForward(messages);
        } catch {
          // If we can't read messages, treat as success (original behavior)
        }

        if (rejected) {
          pushStateStore.setPushError(branch.id, '', true);
        } else {
          pushStateStore.setPushDone(branch.id);
          hasUnpushed = false;
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
    title={pushState === 'pushing'
      ? 'Pushing… (click to view)'
      : pushState === 'error'
        ? 'Push failed — click for details'
        : prState === 'created' && hasUnpushed
          ? optionHeld
            ? 'Force push to remote'
            : 'Push changes to remote'
          : prState === 'created'
            ? 'View PR'
            : prState === 'error'
              ? 'PR creation failed — click for details'
              : prState === 'creating'
                ? 'Creating PR… (click to view)'
                : optionHeld
                  ? 'Create draft PR'
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
    {#if prStatusIndicator}
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
