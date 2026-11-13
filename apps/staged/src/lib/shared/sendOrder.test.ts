import { describe, expect, it } from 'vitest';
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
