import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { SendOrder } from './sendOrder';

/** A request the test settles by hand, recording when it was sent. */
function request<T = void>(log: string[], name: string) {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const settled = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  const send = () => {
    log.push(`${name} sent`);
    return settled;
  };
  return { send, resolve, reject };
}

/** Let every queued promise reaction run. */
async function flush(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe('SendOrder', () => {
  it('sends an uncontended request in the caller’s own tick', () => {
    const order = new SendOrder();
    const log: string[] = [];
    void order.run('note:1', request(log, 'a').send);
    expect(log).toEqual(['a sent']);
  });

  it('holds a request back until the one sent before it for the same key settles', async () => {
    const order = new SendOrder();
    const log: string[] = [];
    const first = request<string>(log, 'first');
    const second = request<string>(log, 'second');
    const third = request<string>(log, 'third');

    const results = [
      order.run('note:1', first.send),
      order.run('note:1', second.send),
      order.run('note:1', third.send),
    ];
    await flush();
    expect(log).toEqual(['first sent']);

    first.resolve('one');
    await flush();
    expect(log).toEqual(['first sent', 'second sent']);

    second.resolve('two');
    third.resolve('three');
    expect(await Promise.all(results)).toEqual(['one', 'two', 'three']);
    expect(log).toEqual(['first sent', 'second sent', 'third sent']);
  });

  it('does not make requests for different keys wait on each other', async () => {
    const order = new SendOrder();
    const log: string[] = [];
    void order.run('note:1', request(log, 'one').send);
    void order.run('note:2', request(log, 'two').send);
    await flush();
    expect(log).toEqual(['one sent', 'two sent']);
  });

  it('sends the next request after a failed one, and reports the failure to its caller only', async () => {
    const order = new SendOrder();
    const log: string[] = [];
    const first = request(log, 'first');
    const second = request<string>(log, 'second');

    const failed = order.run('note:1', first.send);
    const next = order.run('note:1', second.send);
    first.reject(new Error('save failed'));
    await expect(failed).rejects.toThrow('save failed');
    await flush();
    expect(log).toEqual(['first sent', 'second sent']);

    second.resolve('saved');
    expect(await next).toBe('saved');
  });

  it('turns a send that throws into a rejection and still releases the key', async () => {
    const order = new SendOrder();
    const log: string[] = [];
    const thrown = order.run('note:1', () => {
      throw new Error('no transport');
    });
    const next = order.run('note:1', request(log, 'next').send);
    await expect(thrown).rejects.toThrow('no transport');
    await flush();
    expect(log).toEqual(['next sent']);
    void next;
  });

  it('forgets a key once nothing is in flight for it', async () => {
    const order = new SendOrder();
    const log: string[] = [];
    const first = request(log, 'first');
    const pending = order.run('note:1', first.send);
    expect(order.pendingKeys).toBe(1);
    first.resolve();
    await pending;
    await flush();
    expect(order.pendingKeys).toBe(0);

    // The key is idle again, so the next request is sent at once.
    void order.run('note:1', request(log, 'again').send);
    expect(log).toEqual(['first sent', 'again sent']);
  });
});

/**
 * The bound is for the web transport, where a request on a dropped
 * connection never settles. Timers are faked, so "never" costs nothing.
 */
describe('SendOrder with a wait bound', () => {
  let warn: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    vi.useFakeTimers();
    warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.useRealTimers();
    warn.mockRestore();
  });

  it('sends a request once the bound runs out on one ahead that never settles', async () => {
    const order = new SendOrder({ maxWaitMs: 1000 });
    const log: string[] = [];
    const hung = request(log, 'hung');
    const next = request<string>(log, 'next');

    void order.run('tags', hung.send);
    const result = order.run('tags', next.send);
    await vi.advanceTimersByTimeAsync(999);
    expect(log).toEqual(['hung sent']);

    await vi.advanceTimersByTimeAsync(1);
    expect(log).toEqual(['hung sent', 'next sent']);
    expect(warn).toHaveBeenCalledTimes(1);

    // The key is the later request's now: once it settles the key is idle,
    // though the hung request is still out.
    next.resolve('done');
    await expect(result).resolves.toBe('done');
    await vi.advanceTimersByTimeAsync(0);
    expect(order.pendingKeys).toBe(0);
    void order.run('tags', request(log, 'after').send);
    expect(log).toEqual(['hung sent', 'next sent', 'after sent']);
  });

  it('keeps the requests queued behind one hung request in order among themselves', async () => {
    const order = new SendOrder({ maxWaitMs: 1000 });
    const log: string[] = [];
    const second = request(log, 'second');
    const third = request(log, 'third');

    // All four in one tick, as the creates of a multi-file drop are.
    void order.run('tags', request(log, 'hung').send);
    void order.run('tags', second.send);
    void order.run('tags', third.send);
    void order.run('tags', request(log, 'fourth').send);

    // Only the request directly behind the hung one gives up on it.
    await vi.advanceTimersByTimeAsync(1000);
    expect(log).toEqual(['hung sent', 'second sent']);
    expect(warn).toHaveBeenCalledTimes(1);

    // The others still wait for the one sent before them, well inside a
    // bound that restarted when it was sent.
    await vi.advanceTimersByTimeAsync(900);
    expect(log).toEqual(['hung sent', 'second sent']);
    second.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(log).toEqual(['hung sent', 'second sent', 'third sent']);
    await vi.advanceTimersByTimeAsync(900);
    expect(log).toEqual(['hung sent', 'second sent', 'third sent']);
    third.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(log).toEqual(['hung sent', 'second sent', 'third sent', 'fourth sent']);

    expect(warn).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('drains a chain of hung requests, one bound for each', async () => {
    const order = new SendOrder({ maxWaitMs: 1000 });
    const log: string[] = [];
    const last = request<string>(log, 'fourth');

    void order.run('tags', request(log, 'first').send);
    void order.run('tags', request(log, 'second').send);
    void order.run('tags', request(log, 'third').send);
    const result = order.run('tags', last.send);

    // Every request hangs. Each one behind is sent a bound after the one
    // ahead of it was, not all of them a bound after their own call.
    await vi.advanceTimersByTimeAsync(1000);
    expect(log).toEqual(['first sent', 'second sent']);
    await vi.advanceTimersByTimeAsync(999);
    expect(log).toEqual(['first sent', 'second sent']);
    await vi.advanceTimersByTimeAsync(1);
    expect(log).toEqual(['first sent', 'second sent', 'third sent']);
    await vi.advanceTimersByTimeAsync(1000);
    expect(log).toEqual(['first sent', 'second sent', 'third sent', 'fourth sent']);
    expect(warn).toHaveBeenCalledTimes(3);

    // Nothing is left ticking for the three that never answer.
    expect(vi.getTimerCount()).toBe(0);
    last.resolve('done');
    await expect(result).resolves.toBe('done');
    await vi.advanceTimersByTimeAsync(0);
    expect(order.pendingKeys).toBe(0);
  });

  it('counts the wait from the request’s own call when the one ahead is already out', async () => {
    const order = new SendOrder({ maxWaitMs: 1000 });
    const log: string[] = [];

    void order.run('tags', request(log, 'hung').send);
    await vi.advanceTimersByTimeAsync(600);
    void order.run('tags', request(log, 'next').send);

    // 1000 ms after `next` was called, not 1000 ms after `hung` was sent.
    await vi.advanceTimersByTimeAsync(999);
    expect(log).toEqual(['hung sent']);
    await vi.advanceTimersByTimeAsync(1);
    expect(log).toEqual(['hung sent', 'next sent']);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('starts no timer for a request whose predecessor has not been sent yet', async () => {
    const order = new SendOrder({ maxWaitMs: 1000 });
    const log: string[] = [];
    const first = request(log, 'first');

    void order.run('tags', first.send);
    void order.run('tags', request(log, 'second').send);
    void order.run('tags', request(log, 'third').send);
    // One clock: `second` waiting on `first`. `third` waits on a request
    // that is not out yet.
    expect(vi.getTimerCount()).toBe(1);

    first.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(log).toEqual(['first sent', 'second sent']);
    expect(vi.getTimerCount()).toBe(1);
  });

  it('leaves the request it stopped waiting for to finish', async () => {
    const order = new SendOrder({ maxWaitMs: 1000 });
    const log: string[] = [];
    const slow = request<string>(log, 'slow');

    const result = order.run('note:1', slow.send);
    void order.run('note:1', request(log, 'next').send);
    await vi.advanceTimersByTimeAsync(1000);
    expect(log).toEqual(['slow sent', 'next sent']);

    slow.resolve('saved');
    await expect(result).resolves.toBe('saved');
  });

  it('keeps the order and leaves no timer behind when requests settle inside the bound', async () => {
    const order = new SendOrder({ maxWaitMs: 1000 });
    const log: string[] = [];
    const first = request(log, 'first');
    const second = request(log, 'second');

    void order.run('note:1', first.send);
    void order.run('note:1', second.send);
    void order.run('note:1', request(log, 'third').send);
    await vi.advanceTimersByTimeAsync(900);
    expect(log).toEqual(['first sent']);

    first.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(log).toEqual(['first sent', 'second sent']);

    second.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(log).toEqual(['first sent', 'second sent', 'third sent']);
    expect(vi.getTimerCount()).toBe(0);
    expect(warn).not.toHaveBeenCalled();
  });

  it('waits without limit when no bound is given, as under Tauri', async () => {
    const order = new SendOrder();
    const log: string[] = [];

    void order.run('note:1', request(log, 'held').send);
    void order.run('note:1', request(log, 'next').send);
    await vi.advanceTimersByTimeAsync(24 * 60 * 60 * 1000);

    expect(log).toEqual(['held sent']);
    expect(vi.getTimerCount()).toBe(0);
    expect(warn).not.toHaveBeenCalled();
  });
});
