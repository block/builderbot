/**
 * Send-order execution for writes to the same thing.
 *
 * Backend commands run concurrently: always over HTTP, and under Tauri once
 * a command is an `async fn`, which runs on a thread pool. The synchronous
 * commands ran one at a time on the main thread, in the order they were
 * sent, so two writes sent without awaiting the first still landed in that
 * order. On the pool they land in either order: the older of two saves of a
 * note can land last and stay, "mark as unread" can overtake the read mark
 * sent after it, and the second of two queued messages can be queued first.
 *
 * The backend cannot restore the order. It first sees an async command on a
 * runtime worker, after the order of arrival is gone, and HTTP requests never
 * had one. Only the sender knows it, so the sender keeps it: `run(key, send)`
 * holds each request back until the one sent before it for the same key has
 * settled, however it settled. Requests for different keys do not wait on
 * each other.
 *
 * With nothing in flight for the key the request is sent at once, in the
 * caller's own tick, so an uncontended call behaves as it did unqueued. A
 * request that never settles holds back every later one for its key, as a
 * synchronous command that never returned held back every command.
 */
export class SendOrder<K = string> {
  private tails = new Map<K, Promise<void>>();

  /** Send through `send` once every earlier request for `key` has settled. */
  run<T>(key: K, send: () => Promise<T>): Promise<T> {
    const earlier = this.tails.get(key);
    const result = earlier
      ? earlier.then(send)
      : // Inside an executor so a `send` that throws rejects like one that
        // fails later, and still releases the key.
        new Promise<T>((resolve) => resolve(send()));
    const tail = result.then(noop, noop);
    this.tails.set(key, tail);
    void tail.then(() => {
      if (this.tails.get(key) === tail) this.tails.delete(key);
    });
    return result;
  }

  /** How many keys have a request in flight or waiting. */
  get pendingKeys(): number {
    return this.tails.size;
  }
}

function noop(): void {}
