import type { PullState } from '../../stores/pullState.svelte';
import type { PushState } from '../../stores/pushState.svelte';
import type { PipelineExecution } from '../../types';

const IMAGE_EXTENSIONS = ['.png', '.jpg', '.jpeg', '.gif', '.webp'];

/**
 * Whether a push or pull of the branch's own is running or waiting on the branch
 * session queue.
 *
 * Neither creates a timeline artifact, so a timeline-derived "has active sessions"
 * check reads the branch as idle for their whole duration. That skew matters for
 * the gates that ask whether the backend would queue a new action: mid-push, an
 * action they only disable for an *immediate* run (Pull on a dirty worktree) would
 * otherwise stay disabled even though the click would just queue.
 */
export function isGitActionInFlight(args: {
  push?: { state: PushState } | null;
  pull?: { state: PullState } | null;
  /** An immediate pull the card is awaiting inline; it has no store entry. */
  immediatePull?: boolean;
}): boolean {
  if (args.immediatePull) return true;
  const push = args.push?.state;
  const pull = args.pull?.state;
  return push === 'pushing' || push === 'queued' || pull === 'pulling' || pull === 'queued';
}

export const MAX_CONSECUTIVE_POLL_FAILURES = 3;

export type PollFailureTracker = {
  recordSuccess(): void;
  /** Records one failure; true when the budget is exhausted and the poller should give up. */
  recordFailure(): boolean;
};

/**
 * Failure budget for a session poller.
 *
 * `getSession` resolves `null` for a session that no longer exists, so a rejection
 * is always a transport or dispatch failure — the kind that a retry fixes. Tolerate
 * those until several happen in a row; a success resets the budget, so only sustained
 * unreachability, not an isolated blip, makes a poller give up its store entry.
 */
export function createPollFailureTracker(
  maxFailures = MAX_CONSECUTIVE_POLL_FAILURES
): PollFailureTracker {
  let consecutive = 0;
  return {
    recordSuccess(): void {
      consecutive = 0;
    },
    recordFailure(): boolean {
      consecutive += 1;
      return consecutive >= maxFailures;
    },
  };
}

export type PolledSessionDisposition = 'gone' | 'waiting' | 'active' | 'finished';

/**
 * What a session poller should do with the session it just fetched.
 *
 * `getSession` resolves `null` only for a session that is no longer in the table —
 * `delete_session` or the branch-delete cascade on `sessions.branch_id`, since
 * `cancel_session` transitions to `cancelled` instead. So `gone` means the work will
 * never run and there is nothing left to cancel: drop the store entry quietly rather
 * than reporting a failure the user cannot act on. Without this the tick is a no-op
 * and the poller runs forever behind a badge for a session that does not exist.
 *
 * Callers must skip the `'__pending__'` sentinel before polling: it is not a real id,
 * so it classifies as `gone` while the launch command is still in flight.
 */
export function classifyPolledSession(
  session: { status: string } | null | undefined
): PolledSessionDisposition {
  if (!session) return 'gone';
  if (session.status === 'queued') return 'waiting';
  if (session.status === 'running') return 'active';
  return 'finished';
}

/**
 * Cancel a queued git action, clearing its store entry only once the backend
 * confirms.
 *
 * `cancel_session` answers `Ok` for a session it cannot find, one that already
 * finished, and one already cancelled — so a rejection only ever means the request
 * never arrived (a web-mode network blip, the backend restarting, a laptop waking).
 * That is precisely when the queued session still exists, so the badge and its
 * Cancel button have to survive: clearing first would hide a session that goes on
 * to drain, with no affordance left to call it off.
 *
 * The returned function is re-entrant-safe. Leaving the state set while the cancel
 * is in flight means the call sites' own "still queued?" guards no longer stop a
 * second click, so the in-flight flag does. It resets on failure, so retrying by
 * clicking Cancel again works.
 */
export function createQueuedSessionCanceller(deps: {
  cancel: (sessionId: string) => Promise<unknown>;
  clearState: () => void;
  onError: (error: unknown) => void;
}): (sessionId: string) => Promise<boolean> {
  let inFlight = false;
  return async (sessionId: string): Promise<boolean> => {
    if (inFlight) return false;
    inFlight = true;
    try {
      await deps.cancel(sessionId);
      deps.clearState();
      return true;
    } catch (e) {
      deps.onError(e);
      return false;
    } finally {
      inFlight = false;
    }
  };
}

export function getActionTypeLabel(actionType: string): string {
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

export function formatBaseBranch(baseBranch: string): string {
  return baseBranch.replace(/^origin\//, '');
}

function normalizePrUrl(candidate: string): string | null {
  const trimmed = candidate.trim().replace(/^[<`'"\[(]+|[>`'"\]),.?!;:]+$/g, '');
  const match = trimmed.match(
    /^https:\/\/github\.com\/([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+)\/pull\/(\d+)(?:[/?#].*)?$/
  );

  if (!match) return null;

  return `https://github.com/${match[1]}/${match[2]}/pull/${match[3]}`;
}

export function extractPrUrl(messages: { content: string; role: string }[]): string | null {
  for (const msg of messages) {
    if (msg.role !== 'assistant' && msg.role !== 'tool_result') continue;
    const markerMatch = msg.content.match(/PR_URL:\s*(\S+)/);
    if (markerMatch) {
      const normalized = normalizePrUrl(markerMatch[1]);
      if (normalized) return normalized;
    }
  }

  for (const msg of messages) {
    const urlMatches = msg.content.match(/https?:\/\/\S+/g) ?? [];
    for (const url of urlMatches) {
      const normalized = normalizePrUrl(url);
      if (normalized) return normalized;
    }
  }

  return null;
}

export function extractPrNumber(url: string): number | null {
  const match = url.match(/\/pull\/(\d+)/);
  return match ? parseInt(match[1], 10) : null;
}

type MessageLike = { content: string; role: string };

export type CompletedPushOutcome = 'succeeded' | 'rejected_non_fast_forward';

function containsNonFastForwardPushMarker(content: string): boolean {
  const lowerContent = content.toLowerCase();
  return (
    content.includes('PUSH_REJECTED: NON_FAST_FORWARD') || lowerContent.includes('non-fast-forward')
  );
}

export function isPushRejectedNonFastForward(messages: MessageLike[]): boolean {
  return messages.some(
    (msg) =>
      (msg.role === 'assistant' || msg.role === 'tool_result') &&
      containsNonFastForwardPushMarker(msg.content)
  );
}

export function classifyPipelinePushCompletion(
  pipeline: PipelineExecution | null | undefined,
  messages?: MessageLike[]
): CompletedPushOutcome | null {
  if (!pipeline) return null;

  const hasNonFastForward = pipeline.steps.some(
    (step) =>
      step.status === 'failed' &&
      step.output !== null &&
      containsNonFastForwardPushMarker(step.output)
  );
  if (hasNonFastForward) {
    // If AI ran after the pipeline failure (e.g. a force-push that failed with
    // --force-with-lease and the error happened to contain "non-fast-forward"),
    // the AI handled recovery — don't classify as rejected. Only treat it as
    // rejected when no AI session ran (the pipeline aborted immediately).
    const aiRan = messages && messages.some((m) => m.role === 'assistant');
    if (aiRan) return 'succeeded';
    return 'rejected_non_fast_forward';
  }

  const allStepsPassedOrSkipped = pipeline.steps.every(
    (step) => step.status === 'succeeded' || step.status === 'skipped'
  );
  if (pipeline.completedWithoutAi || allStepsPassedOrSkipped) return 'succeeded';

  return null;
}

export function classifyCompletedPushSession(
  pipeline: PipelineExecution | null | undefined,
  messages: MessageLike[]
): CompletedPushOutcome {
  return (
    classifyPipelinePushCompletion(pipeline, messages) ??
    (isPushRejectedNonFastForward(messages) ? 'rejected_non_fast_forward' : 'succeeded')
  );
}

export function isImageFile(filePath: string): boolean {
  const lower = filePath.toLowerCase();
  return IMAGE_EXTENSIONS.some((ext) => lower.endsWith(ext));
}

export function isMaybeTextFile(filePath: string): boolean {
  return !isImageFile(filePath);
}

/**
 * Insert file paths into a textarea at the cursor position, preserving undo history.
 * Uses `document.execCommand('insertText')` which, while deprecated, is the simplest
 * way to get undo-friendly insertion in Chromium/Tauri webviews.
 */
export function insertFilePathsAtCursor(element: HTMLElement, paths: string[]): void {
  const insert = paths.join('\n');
  element.focus();
  document.execCommand('insertText', false, insert);
}

export function fileNameFromPath(filePath: string): string {
  const parts = filePath.split('/');
  const name = parts[parts.length - 1] || filePath;
  return name.replace(/\.[^.]+$/, '');
}
