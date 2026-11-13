/**
 * Latest-wins ordering for responses to the same read.
 *
 * Backend reads run concurrently: always over HTTP, and under Tauri since the
 * hot store reads moved off the main thread, where they used to run one at a
 * time and answer in the order they were sent. Two requests for the same
 * thing can now land in either order. A request sent later was sent after
 * whatever change prompted it, so its response is at least as fresh as that
 * change; a response that lands after a later-sent one has been applied would
 * put older state back, and nothing would refetch it.
 *
 * Take a ticket with `issue()` when sending, and apply the response only if
 * `admit(key, ticket)` allows it. Responses that land in the order they were
 * sent are all admitted. Tickets come from one counter that never resets, so
 * tickets issued for different keys can be compared with `overtaken`.
 *
 * This orders reads against reads only. State that is also written locally,
 * from the result of a write, needs one more step: a read sent before that
 * write landed may predate it, and nothing above refuses it, since no later
 * read has landed. Record the local write as a landing under a key of its
 * own (`wrote(writeKey)`), and have a read that finds itself
 * `overtaken(writeKey, ticket)` read again. Reading again, where dropping
 * would be simpler, keeps whatever else the dropped response carried. Only
 * drop when the local write replaced the whole of the state.
 */
export class LatestWins<K = string> {
  /** The last place handed out in the order, to a request or a local write. */
  private sent = 0;
  /** The ticket of the last request sent. */
  private newestRequest = 0;
  private landed = new Map<K, number>();

  /** A ticket for a request about to be sent. */
  issue(): number {
    this.newestRequest = ++this.sent;
    return this.newestRequest;
  }

  /**
   * Record a local write as a landing for `key`, placed after every request
   * sent so far and before every one sent from now on. It is not a request:
   * `isNewest` still answers for the last request sent, which this write may
   * be about to send back for another read.
   */
  wrote(key: K): void {
    this.landed.set(key, ++this.sent);
  }

  /**
   * Record that the response for `ticket` landed for `key`. False when a
   * response sent after it has already landed there: this one is stale and
   * must not be applied.
   */
  admit(key: K, ticket: number): boolean {
    if (this.overtaken(key, ticket)) return false;
    this.landed.set(key, ticket);
    return true;
  }

  /** Whether a response sent after `ticket` has already landed for `key`. */
  overtaken(key: K, ticket: number): boolean {
    return (this.landed.get(key) ?? 0) > ticket;
  }

  /**
   * Whether no request has been sent since the one `ticket` stands for. For
   * state that describes the requests still out, such as a spinner, which an
   * older request settling must not clear. Local writes (`wrote`) do not
   * count: nothing of theirs is out, so a write landing while the newest
   * request is still out must not leave that state set for good.
   */
  isNewest(ticket: number): boolean {
    return ticket === this.newestRequest;
  }
}
