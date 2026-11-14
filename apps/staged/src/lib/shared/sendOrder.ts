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
 * caller's own tick, so an uncontended call behaves as it did unqueued.
 *
 * A request that never settles holds back every later one for its key. Under
 * Tauri that is the old behaviour in miniature: a synchronous command that
 * never returned held back every command. Over HTTP it is new. Requests there
 * were independent before they were ordered, and a `fetch` on a dropped
 * connection can stay pending for as long as the page is open, which would
 * stop every later write for the key until a reload.
 *
 * `maxWaitMs` bounds that. A request waits at most that long for the one
 * ahead of it, and is then sent beside it. The request it stopped waiting for
 * is not cancelled or failed, so a command that is only slow still completes;
 * the two can then land in either order, as every pair did over HTTP before.
 *
 * The clock runs only while the one ahead is out: it starts at the call to
 * `run`, or when the one ahead is sent if that is later. Requests queued
 * behind one hung request therefore do not all run out together. The first
 * gives up and is sent; the second then waits for that send, with a full
 * bound of its own, and so on, so the requests that are not hung keep their
 * order among themselves. A chain still drains, one bound for each request in
 * it that hangs.
 *
 * Leave `maxWaitMs` unset under Tauri, where the wait must have no bound: a
 * store lock held for longer than any bound would otherwise let a later
 * write overtake an earlier one.
 */
export class SendOrder<K = string> {
  private tails = new Map<K, InFlight>();
  private readonly maxWaitMs: number | undefined;

  constructor({ maxWaitMs }: { maxWaitMs?: number } = {}) {
    this.maxWaitMs = maxWaitMs;
  }

  /** Send through `send` once every earlier request for `key` has settled. */
  run<T>(key: K, send: () => Promise<T>): Promise<T> {
    const earlier = this.tails.get(key);
    const request: InFlight = { settled: SETTLED, sent: false, onSent: [] };
    const sendNow = (): Promise<T> => {
      request.sent = true;
      // Only a bounded wait registers here, so this is empty under Tauri.
      for (const startClock of request.onSent.splice(0)) startClock();
      return send();
    };
    const result = earlier
      ? this.settledOrOverdue(key, earlier).then(sendNow)
      : // Inside an executor so a `send` that throws rejects like one that
        // fails later, and still releases the key.
        new Promise<T>((resolve) => resolve(sendNow()));
    request.settled = result.then(noop, noop);
    this.tails.set(key, request);
    void request.settled.then(() => {
      if (this.tails.get(key) === request) this.tails.delete(key);
    });
    return result;
  }

  /** How many keys have a request in flight or waiting. */
  get pendingKeys(): number {
    return this.tails.size;
  }

  /**
   * `earlier.settled` itself when the wait has no bound, so that path gains
   * no timer and no extra tick. With a bound, a promise that resolves when
   * `earlier` settles, or when it has been out for the bound since this call,
   * whichever is first. While `earlier` is itself still waiting to be sent
   * there is no timer: the wait for it is bounded by the timers ahead.
   */
  private settledOrOverdue(key: K, earlier: InFlight): Promise<void> {
    const maxWaitMs = this.maxWaitMs;
    if (maxWaitMs === undefined) return earlier.settled;
    return new Promise<void>((resolve) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const startClock = () => {
        timer = setTimeout(() => {
          timer = undefined;
          console.warn(
            `[sendOrder] A write for "${String(key)}" has waited ${maxWaitMs} ms for the one ` +
              'sent before it to answer; sending it without waiting any longer.'
          );
          resolve();
        }, maxWaitMs);
      };
      if (earlier.sent) startClock();
      else earlier.onSent.push(startClock);
      // `earlier` cannot settle before it is sent, so by now the clock has
      // either started or already run out.
      void earlier.settled.then(() => {
        if (timer !== undefined) clearTimeout(timer);
        resolve();
      });
    });
  }
}

/** A request `run` has taken: waiting for the ones ahead of it, or sent. */
interface InFlight {
  /** Resolves once the request has settled, however it settled. */
  settled: Promise<void>;
  /** Whether `send` has been called for it. */
  sent: boolean;
  /** Called when it is sent: the clocks of the bounded waits behind it. */
  onSent: Array<() => void>;
}

const SETTLED: Promise<void> = Promise.resolve();

function noop(): void {}
