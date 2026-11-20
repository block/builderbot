import {
  RECONCILE_NO_WORKSPACE,
  RECONCILE_PROMPT_NOT_RECEIVED,
  RECONCILE_TURN_UNDETERMINED,
  RECONCILE_WORKSTATION_GONE,
} from '../../shared/disconnectedSession';

export type ArtifactKind = 'commit' | 'note' | 'review';

const artifactNoun: Record<ArtifactKind, string> = {
  commit: 'commit',
  note: 'note',
  review: 'comments',
};

/**
 * Reconcile messages on a Blox session Staged ended because it couldn't
 * follow the turn, not because anyone stopped it: the row's `interrupted`
 * reason would otherwise read as a user Stop.
 */
const RECONCILE_INTERRUPTIONS: ReadonlySet<string> = new Set([
  RECONCILE_TURN_UNDETERMINED,
  RECONCILE_NO_WORKSPACE,
  RECONCILE_PROMPT_NOT_RECEIVED,
]);

/**
 * The `rateLimitKind` of a `rate_limited` pause whose turn failed because the
 * agent could not reach its model provider, not because of an API limit
 * (`NETWORK_PAUSE_KIND` in `src-tauri/src/rate_limit_scheduler.rs`).
 */
export const NETWORK_PAUSE_KIND = 'network';

/**
 * The `reason` in a network pause's retry bookkeeping while it waits behind
 * the scheduler's network gate (`NETWORK_WAIT_REASON` in
 * `src-tauri/src/rate_limit_scheduler.rs`). Such a row's `nextRetryAt` is the
 * time it paused, not the time of its next attempt.
 */
export const NETWORK_WAIT_REASON = 'network_wait';

/** What a network pause says wherever it is named: the row subtitle and the chat alert. */
export const NETWORK_PAUSED = 'Paused: network unavailable.';
/**
 * The same, with the promise of an automatic resume. Only for a caller that
 * has the scheduler's bookkeeping and so knows the promise holds.
 */
export const NETWORK_PAUSED_RESUMES = `${NETWORK_PAUSED} Resumes when the connection returns.`;

/** Whether a `rate_limited` pause with this `rateLimitKind` is a network pause. */
export function isNetworkPause(rateLimitKind: string | null | undefined): boolean {
  return rateLimitKind === NETWORK_PAUSE_KIND;
}

/** The session fields `failedArtifactSubtitle` reads beyond the completion reason. */
export interface FailedArtifactDetails {
  /** The session's error: reconcile's messages say more than its completion reason does. */
  errorMessage?: string | null;
  /** Dates an API-limit pause's subtitle. */
  rateLimitResetsAt?: string | null;
  /** Tells a network pause from an API-limit pause. */
  rateLimitKind?: string | null;
  /** The note row's `NoteTimelineItem.isPollingForNote`. */
  isPollingForNote?: boolean;
}

/**
 * The subtitle of a timeline row whose session ended without producing its
 * artifact.
 */
export function failedArtifactSubtitle(
  completionReason: string | null | undefined,
  kind: ArtifactKind,
  {
    errorMessage,
    rateLimitResetsAt,
    rateLimitKind,
    isPollingForNote = false,
  }: FailedArtifactDetails = {}
): string {
  const noun = artifactNoun[kind];
  if (errorMessage === RECONCILE_WORKSTATION_GONE) {
    return `Workstation deleted — no ${noun} created`;
  }
  if (errorMessage && RECONCILE_INTERRUPTIONS.has(errorMessage)) {
    return `Session interrupted — no ${noun} created`;
  }
  switch (completionReason) {
    case 'crashed':
      return `Session crashed — no ${noun} created`;
    case 'remote_agent_lost':
      return `Workspace agent lost — no ${noun} created`;
    case 'rate_limited':
      // A timeline row does not carry the scheduler's bookkeeping, so it
      // cannot tell a pause that will resume by itself from one the
      // scheduler gave up on or will not resume (a pending tool call). It
      // makes no promise either way; the chat alert, which can tell, does.
      if (isNetworkPause(rateLimitKind)) return NETWORK_PAUSED;
      return rateLimitPausedSubtitle(rateLimitResetsAt) ?? `Paused by API limit — no ${noun} yet`;
    case 'app_quit':
      return `Session interrupted — no ${noun} created`;
    // A detached remote session may still complete on its own while the
    // sweep is polling the workstation or an instance is storing the note it
    // found (`NoteTimelineItem.isPollingForNote`, see docs/note-delivery.md).
    // Past the poll window, or for a note that cannot be stored, nothing
    // arrives without a Resume, so "yet" would be a false promise.
    case 'detached':
      return isPollingForNote
        ? `Lost the remote session — no ${noun} created yet`
        : `Lost the remote session — no ${noun} created`;
    case 'project_session_interrupted':
      return `Session stopped by project session — no ${noun} created`;
    case 'interrupted':
      return `Session stopped — no ${noun} created`;
    case 'held_until_cap':
    case 'hold_stopped':
      return `Session stopped waiting on background work — no ${noun} created`;
    default:
      return `Session finished — no ${noun} created`;
  }
}

export function rateLimitPausedSubtitle(rateLimitResetsAt?: string | null): string | null {
  const resetTime = formatRateLimitResetTime(rateLimitResetsAt);
  if (resetTime) return `Paused by API limit — resets around ${resetTime}`;
  return null;
}

export function formatRateLimitResetTime(rateLimitResetsAt?: string | null): string | null {
  if (!rateLimitResetsAt) return null;
  const date = new Date(rateLimitResetsAt);
  if (Number.isNaN(date.getTime())) return null;

  return date.toLocaleTimeString([], {
    hour: 'numeric',
    minute: '2-digit',
  });
}
