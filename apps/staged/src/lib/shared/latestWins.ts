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
 * own (`admit(writeKey, issue())`), and have a read that finds itself
 * `overtaken(writeKey, ticket)` read again. Reading again, where dropping
 * would be simpler, keeps whatever else the dropped response carried. Only
 * drop when the local write replaced the whole of the state.
 */
export class LatestWins<K = string> {
  private sent = 0;
  private landed = new Map<K, number>();

  /** A ticket for a request about to be sent. */
  issue(): number {
    return ++this.sent;
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
   * Whether no ticket has been issued since `ticket`: the request it stands
   * for is the newest one sent. For state that describes the requests still
   * out, such as a spinner, which an older request settling must not clear.
   */
  isNewest(ticket: number): boolean {
    return ticket === this.sent;
  }
}
