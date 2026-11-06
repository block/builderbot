/**
 * Copy and state helpers for `disconnected` sessions: Blox sessions that lost
 * their connection mid-turn, whose agent may still be working on the
 * workstation while the backend's reconcile service tries to catch up.
 *
 * Shared by the branch timeline, the projects sidebar and grid, child-session
 * badges and the session chat pane, so a disconnected session reads the same
 * everywhere.
 */

import type {
  SessionReconcileEvent,
  SessionReconcileReason,
  SessionReconcileReasonKind,
} from '../types';

// -----------------------------------------------------------------------------
// Terminal messages reconcile writes
// -----------------------------------------------------------------------------
//
// Mirrors the `error_message` constants in
// `src-tauri/src/session_runner/reconcile/{row,stop}.rs`. Matched exactly: the
// frontend only reframes these, so an unrecognised message still renders as
// written.

/** `cancelled`: a Stop the workstation didn't confirm. */
export const RECONCILE_MAY_STILL_RUN =
  'Stopped in Staged, but the turn may still be running on the workstation';
/** `cancelled`/`interrupted`, resumable: nothing said whether the turn finished. */
export const RECONCILE_TURN_UNDETERMINED =
  "Staged couldn't tell whether this turn finished on the workstation";
/** `cancelled`/`interrupted`: the row's branch has no workstation to ask. */
export const RECONCILE_NO_WORKSPACE = "Staged couldn't find this session's workstation";
/**
 * `cancelled`/`interrupted`, resumable: the connection went before the newest
 * message reached the agent, so the turn that ended was an earlier one.
 */
export const RECONCILE_PROMPT_NOT_RECEIVED =
  'Staged lost its connection before this message reached the workstation';
/** `error`/`crashed`: the workstation was deleted. */
export const RECONCILE_WORKSTATION_GONE = 'The workstation no longer exists';

/**
 * Reconcile messages on a `cancelled` row that explain an ending rather than
 * report a failure: they render like any other stopped session's notice (with
 * Resume, since these rows are resumable), not as an error.
 */
const RECONCILE_CANCEL_NOTICES: ReadonlySet<string> = new Set([
  RECONCILE_MAY_STILL_RUN,
  RECONCILE_TURN_UNDETERMINED,
  RECONCILE_NO_WORKSPACE,
  RECONCILE_PROMPT_NOT_RECEIVED,
]);

/** Whether a `cancelled` row's message is one of reconcile's explanatory notices. */
export function isReconcileCancelNotice(
  status: string | null | undefined,
  errorMessage: string | null | undefined
): boolean {
  return status === 'cancelled' && !!errorMessage && RECONCILE_CANCEL_NOTICES.has(errorMessage);
}

// -----------------------------------------------------------------------------
// Disconnected state copy
// -----------------------------------------------------------------------------

/** The state every surface leads with while a session is `disconnected`. */
export const DISCONNECTED_STATE = 'Running on Blox';

/** The chat pane's explanation of a `disconnected` session. */
export const DISCONNECTED_ALERT_MESSAGE =
  'Staged lost its connection to the workstation. The agent may still be working; Staged will catch up when it can reach Blox.';

/** Short form of each reason, after "Running on Blox — ". */
const REASON_PHRASE: Record<SessionReconcileReasonKind, string> = {
  unreachable: 'can’t reach Blox',
  not_authenticated: 'signed out of Blox',
  suspended: 'workstation suspended',
  workstation_not_running: 'workstation not running',
  attach_failed: 'couldn’t reconnect',
  background_work: 'background work still running',
};

/** Sentence form of each reason, for the chat pane. */
const REASON_SENTENCE: Record<SessionReconcileReasonKind, string> = {
  unreachable: 'Staged can’t reach Blox right now.',
  not_authenticated: 'Staged is signed out of Blox.',
  suspended: 'The workstation is suspended. Staged will catch up once it’s running again.',
  workstation_not_running: 'The workstation isn’t running.',
  attach_failed: 'Staged reached the workstation but couldn’t reconnect to this session.',
  background_work:
    'The turn has finished, but background work it started is still running on the workstation. Staged will catch up once it ends.',
};

/** The phrase after "Running on Blox — " for where reconciling stands. */
function statePhrase(state: SessionReconcileEvent | null | undefined): string {
  switch (state?.phase) {
    case 'checking':
      return 'catching up…';
    case 'stopping':
      return 'stopping…';
    case 'waiting':
      return state.reason ? REASON_PHRASE[state.reason.kind] : 'reconnecting';
    default:
      return 'reconnecting';
  }
}

/**
 * One-line state for a `disconnected` session, e.g. "Running on Blox —
 * reconnecting" or "Running on Blox — can't reach Blox". Shown as a timeline
 * row's meta and as the sidebar's tooltip.
 */
export function disconnectedLabel(state: SessionReconcileEvent | null | undefined): string {
  return `${DISCONNECTED_STATE} — ${statePhrase(state)}`;
}

/** `disconnectedLabel` for the sidebar's lowercase activity line. */
export function disconnectedActivity(state: SessionReconcileEvent | null | undefined): string {
  return `running on Blox — ${statePhrase(state)}`;
}

/**
 * Which of a project's disconnected sessions speaks for it: one in progress
 * (checking or stopping) first, then one with a reason, else any. Absent
 * states read as plain "reconnecting".
 */
export function representativeReconcileState(
  states: readonly (SessionReconcileEvent | null | undefined)[]
): SessionReconcileEvent | null {
  const known = states.filter((s): s is SessionReconcileEvent => !!s);
  return (
    known.find((s) => s.phase === 'checking' || s.phase === 'stopping') ??
    known.find((s) => s.phase === 'waiting' && s.reason) ??
    known[0] ??
    null
  );
}

/** How the chat pane's disconnected alert describes where reconciling stands. */
export interface DisconnectedAlertStatus {
  /** A check or a stop is in progress: show a spinner, hold the buttons. */
  busy: 'checking' | 'stopping' | null;
  /** The status line under the explanation, if any. */
  text: string | null;
  /** Raw detail for the status line's tooltip (e.g. the `sq` error). */
  detail: string | null;
}

/**
 * The alert's status line. `checkRequested` covers the gap between a "Check
 * now" click and the backend's first `checking` event.
 */
export function disconnectedAlertStatus(
  state: SessionReconcileEvent | null | undefined,
  nowMs: number,
  checkRequested = false
): DisconnectedAlertStatus {
  if (state?.phase === 'stopping') {
    return { busy: 'stopping', text: 'Stopping the turn on the workstation…', detail: null };
  }
  if (state?.phase === 'checking' || checkRequested) {
    return { busy: 'checking', text: 'Catching up with the workstation…', detail: null };
  }
  if (state?.phase === 'waiting' && state.reason) {
    const next = nextCheckPhrase(state.nextAttemptAt, nowMs);
    return {
      busy: null,
      text: next ? `${reasonSentence(state.reason)} ${next}` : reasonSentence(state.reason),
      detail: state.reason.message || null,
    };
  }
  return { busy: null, text: null, detail: null };
}

function reasonSentence(reason: SessionReconcileReason): string {
  return REASON_SENTENCE[reason.kind] ?? 'Staged couldn’t reach the workstation.';
}

/** "Next check in 4 min." for a pending retry, or null when none is scheduled. */
export function nextCheckPhrase(
  nextAttemptAt: number | null | undefined,
  nowMs: number
): string | null {
  if (nextAttemptAt == null) return null;
  const minutes = Math.ceil((nextAttemptAt - nowMs) / 60_000);
  if (minutes <= 1) return 'Next check in under a minute.';
  return `Next check in ${minutes} min.`;
}
