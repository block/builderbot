/**
 * liveDelivery.ts — What the chat pane remembers about whether a running
 * session can take a message now, and when it asks the backend again.
 *
 * The backend answers `get_session_live_delivery` from a registry lookup, so
 * asking is cheap, but asking for a whole run is pointless when the answer
 * cannot change: a session another Staged instance owns never gets a
 * connection in this process. The backend marks such a "no" as not
 * `retryable`; a "no" while the connection is still coming up is. The pane
 * asks until it has a yes or a non-retryable no, and forgets everything when
 * the status moves or the pane shows a different session.
 *
 * It asks when an update of the session arrives, and again
 * `LIVE_DELIVERY_RETRY_MS` after a retryable no. The second is what makes
 * the answer independent of the session stream: a frame arrives only when
 * something is written, and the connection can come up with nothing written
 * after it for a while.
 */
import type { SessionLiveDelivery, SessionStatus } from '../../types';

export interface LiveDeliveryKnowledge {
  /** The backend said the running session can take a message now. */
  supported: boolean;
  /**
   * Nothing to ask until the status moves: a yes, or a no that cannot change
   * during this run. False means the next update or retry asks (again).
   */
  settled: boolean;
}

/**
 * How long after a retryable no the pane asks again when no update of the
 * session has asked first. The connection usually comes up within a few
 * seconds of a run starting.
 */
export const LIVE_DELIVERY_RETRY_MS = 500;

/** Nothing known yet: the next update while running asks. */
export const LIVE_DELIVERY_UNKNOWN: LiveDeliveryKnowledge = Object.freeze({
  supported: false,
  settled: false,
});

/** Whether to ask the backend now. */
export function shouldAskLiveDelivery(
  status: SessionStatus | undefined,
  knowledge: LiveDeliveryKnowledge
): boolean {
  return status === 'running' && !knowledge.settled;
}

/** What to remember from the backend's answer. */
export function liveDeliveryKnowledge(capability: SessionLiveDelivery): LiveDeliveryKnowledge {
  if (capability.supported) return { supported: true, settled: true };
  return { supported: false, settled: !capability.retryable };
}
