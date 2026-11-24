/**
 * Which status an open chat shows while it is fed by the session stream.
 *
 * A session's status reaches the pane by two routes that are not ordered with
 * each other: the `session-status-changed` event, emitted by whoever wrote
 * the status, and the session row of a stream frame. A frame read just before
 * a status write can arrive after that write's event, and its row must not
 * put the older status back.
 *
 * The poll path settles this with a counter (a status event during a fetch
 * discards the fetch's session row). A frame has no fetch to bracket, and
 * the event carries nothing a frame can be ordered against. What the stream
 * guarantees instead (`session_stream/full.rs`): every write to the session
 * row is followed by a frame carrying the row as read after it, unless the
 * client was already sent exactly that row, and rows arrive in the order
 * they were read. So the frame that agrees with an event always comes, and
 * comes last.
 *
 * The rule here follows from that. An event's status is held over the row of
 * any frame that disagrees with it, and the status is taken from frames
 * again from the first row that agrees. An event the frames had already
 * caught up with (the last row they brought has its status) holds nothing,
 * so a frame that overtook its event is not then mistaken for an older one.
 *
 * A held status is the pane's: the frame's other fields are still adopted,
 * since the frame that agrees may never differ in them and so never be sent.
 *
 * The agreeing frame can fail to come in one case: another Staged process
 * rewrote the row between this process's write and the hub's read of it, so
 * the hub only ever read the newer row, and that process's event is emitted
 * where this page cannot hear it. A hold therefore ends by itself after
 * `STATUS_HOLD_MS`; the pane then asks the stream for the row as it is now.
 */

import type { Session, SessionStatus } from '../../types';

/**
 * How long an event's status is held with no frame agreeing. Far longer than
 * a frame takes (the hub sends within a few hundred milliseconds of a write).
 */
export const STATUS_HOLD_MS = 5_000;

export class StatusHold {
  /** The status of the last event, while no frame has agreed with it. */
  #held: SessionStatus | null = null;
  /** The status of the last session row a frame brought. */
  #frameStatus: SessionStatus | null = null;

  /** Whether an event's status is being held over the frames'. */
  get holding(): boolean {
    return this.#held !== null;
  }

  /**
   * A status event was applied to the pane's session. Returns whether its
   * status is now held, so the caller can time the hold.
   */
  event(status: SessionStatus): boolean {
    this.#held = this.#frameStatus === status ? null : status;
    return this.holding;
  }

  /**
   * The session row to adopt for a frame that brought `row`, given the row
   * the pane shows: the frame's own, or the frame's with the pane's status
   * (and the error and completion reason that came with it) while an event's
   * status is held.
   */
  frame(row: Session, shown: Session | null): Session {
    this.#frameStatus = row.status;
    if (this.#held === null) return row;
    if (row.status === this.#held) {
      this.#held = null;
      return row;
    }
    if (!shown) return row;
    return {
      ...row,
      status: shown.status,
      errorMessage: shown.errorMessage,
      completionReason: shown.completionReason,
    };
  }

  /** Stop holding. Returns whether a status was held. */
  release(): boolean {
    const held = this.holding;
    this.#held = null;
    return held;
  }

  /** Forget everything: the pane starts on a stream it has read nothing from. */
  reset(): void {
    this.#held = null;
    this.#frameStatus = null;
  }
}
