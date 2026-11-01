/**
 * liveDelivery.ts — What the chat pane remembers about whether a running
 * session can take a message now, and when it asks the backend again.
 *
 * The backend answers `get_session_live_delivery` from a registry lookup, so
 * asking is cheap, but asking at every poll tick for a whole run is pointless
 * when the answer cannot change: a session another Staged instance owns never
 * gets a connection in this process. The backend marks such a "no" as not
 * `retryable`; a "no" while the connection is still coming up is. The pane
 * asks until it has a yes or a non-retryable no, and forgets everything when
 * the status moves or the pane shows a different session.
 */
import type { SessionLiveDelivery, SessionStatus } from '../../types';

export interface LiveDeliveryKnowledge {
  /** The backend said the running session can take a message now. */
  supported: boolean;
  /**
   * Nothing to ask until the status moves: a yes, or a no that cannot change
   * during this run. False means the next poll tick asks (again).
   */
  settled: boolean;
}

/** Nothing known yet: the next tick while running asks. */
export const LIVE_DELIVERY_UNKNOWN: LiveDeliveryKnowledge = Object.freeze({
  supported: false,
  settled: false,
});

/** Whether this poll tick should ask the backend. */
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
