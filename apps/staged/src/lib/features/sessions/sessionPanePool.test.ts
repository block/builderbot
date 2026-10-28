import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  PaneLease,
  PaneProps,
  SessionPaneExports,
  SessionPanePool,
} from './sessionPanePool.svelte';

// vitest has no Svelte plugin: the component the default pool mounts cannot
// be loaded, and the rune in the pool's props class is stubbed as identity
// (preferences.test.ts precedent). `$state.raw` is the one the class uses.
vi.mock('./SessionChatPane.svelte', () => ({ default: function SessionChatPane() {} }));
vi.stubGlobal(
  '$state',
  Object.assign((value: unknown) => value, { raw: (value: unknown) => value })
);

/** A host the pool can `remove()` and an adopter can append, without a DOM. */
type FakeHost = { parentNode: FakeSlot | null; remove(): void };
type FakeSlot = { children: FakeHost[]; appendChild(host: FakeHost): void };
function slot(): FakeSlot {
  return {
    children: [],
    appendChild(host) {
      host.parentNode = this;
      this.children.push(host);
    },
  };
}

type FakePane = SessionPaneExports & { id: number; props: PaneProps };

let mounted: FakePane[];
let unmounted: FakePane[];
let idle: Array<() => void>;
let pool: SessionPanePool;
const context = new Map<unknown, unknown>();

const flushMicrotasks = () => new Promise<void>((resolve) => queueMicrotask(resolve));
const runIdle = () => {
  const cbs = idle.splice(0);
  for (const cb of cbs) cb();
};

beforeEach(async () => {
  vi.resetModules();
  const { createSessionPanePool } = await import('./sessionPanePool.svelte');
  mounted = [];
  unmounted = [];
  idle = [];
  let nextId = 1;
  pool = createSessionPanePool({
    capacity: 2,
    createHost: () => {
      const host: FakeHost = {
        parentNode: null,
        remove() {
          if (this.parentNode) {
            this.parentNode.children = this.parentNode.children.filter((h) => h !== this);
          }
          this.parentNode = null;
        },
      };
      return host as unknown as HTMLDivElement;
    },
    mount: (_host, props) => {
      const pane: FakePane = {
        id: nextId++,
        props,
        park: vi.fn(),
        unpark: vi.fn(),
        close: vi.fn(),
        clearSearch: vi.fn(),
        performSearch: vi.fn(),
        nextMatch: vi.fn(),
        previousMatch: vi.fn(),
      };
      mounted.push(pane);
      return pane;
    },
    unmount: (pane) => {
      unmounted.push(pane as FakePane);
    },
    scheduleIdle: (cb) => {
      idle.push(cb);
    },
  });
});

/** Acquire, attach to a slot and wait for the pane, the way the adopter does. */
async function adopt(sessionId: string, into = slot()) {
  const lease = pool.acquire(sessionId, context);
  into.appendChild(lease.host as unknown as FakeHost);
  let pane: FakePane | null = null;
  lease.ready((p) => {
    pane = p as FakePane;
  });
  await flushMicrotasks();
  return { lease, pane: pane as FakePane | null, slot: into };
}

describe('hits and misses', () => {
  it('mounts once on a miss, from a microtask, into the attached host', async () => {
    const into = slot();
    const lease = pool.acquire('s1', context);
    expect(mounted).toHaveLength(0);
    into.appendChild(lease.host as unknown as FakeHost);
    const ready = vi.fn();
    lease.ready(ready);
    expect(ready).not.toHaveBeenCalled();
    await flushMicrotasks();
    expect(mounted).toHaveLength(1);
    expect(ready).toHaveBeenCalledWith(mounted[0]);
    expect((lease.host as unknown as FakeHost).parentNode).toBe(into);
  });

  it('reuses the same instance and host on a hit, and reports it synchronously', async () => {
    const first = await adopt('s1');
    first.lease.release();
    const lease = pool.acquire('s1', context);
    const ready = vi.fn();
    lease.ready(ready);
    expect(ready).toHaveBeenCalledWith(first.pane);
    expect(lease.host).toBe(first.lease.host);
    expect(lease.props).toBe(first.lease.props);
    expect(lease.transient).toBe(false);
    await flushMicrotasks();
    expect(mounted).toHaveLength(1);
  });

  it('evicts only free entries, least recently used first, beyond capacity', async () => {
    const a = await adopt('a');
    const b = await adopt('b');
    a.lease.release();
    b.lease.release();
    // Touch `a` so `b` is the least recently used free entry.
    (await adopt('a')).lease.release();
    await adopt('c');
    expect(pool.pooledSessionIds()).toEqual(['a', 'c']);
    runIdle();
    expect(unmounted).toEqual([b.pane]);

    // Both remaining entries leased: nothing can be evicted, the pool grows.
    const a2 = await adopt('a');
    const d = await adopt('d');
    expect(pool.pooledSessionIds()).toEqual(['c', 'a', 'd']);
    runIdle();
    expect(unmounted).toEqual([b.pane]);
    a2.lease.release();
    d.lease.release();
  });

  it('unmounts an evicted pane through scheduleIdle, never synchronously', async () => {
    const a = await adopt('a');
    a.lease.release();
    (await adopt('b')).lease.release();
    await adopt('c');
    expect(unmounted).toEqual([]);
    expect(idle).toHaveLength(1);
    runIdle();
    expect(unmounted).toEqual([a.pane]);
  });
});

describe('release', () => {
  it('parks the pane before detaching the host', async () => {
    const { lease, pane } = await adopt('s1');
    const order: string[] = [];
    (pane!.park as ReturnType<typeof vi.fn>).mockImplementation(() => {
      order.push(
        `park:${(lease.host as unknown as FakeHost).parentNode ? 'attached' : 'detached'}`
      );
    });
    lease.release();
    expect(order).toEqual(['park:attached']);
    expect((lease.host as unknown as FakeHost).parentNode).toBeNull();
  });

  it('clears the callback props and turns active off', async () => {
    const { lease } = await adopt('s1');
    lease.props.active = true;
    lease.props.onSessionChange = vi.fn();
    lease.props.onSearchStateChange = vi.fn();
    lease.props.onOpenSession = vi.fn();
    lease.props.onHashtagClick = vi.fn();
    lease.release();
    expect(lease.props.active).toBe(false);
    expect(lease.props.onSessionChange).toBeUndefined();
    expect(lease.props.onSearchStateChange).toBeUndefined();
    expect(lease.props.onOpenSession).toBeUndefined();
    expect(lease.props.onHashtagClick).toBeUndefined();
  });

  it('is idempotent', async () => {
    const { lease, pane } = await adopt('s1');
    lease.release();
    lease.release();
    expect(pane!.park).toHaveBeenCalledTimes(1);
    expect(pool.pooledSessionIds()).toEqual(['s1']);
  });

  it('does not run a released lease’s ready callback when the mount lands later', async () => {
    const lease = pool.acquire('s1', context);
    const ready = vi.fn();
    lease.ready(ready);
    lease.release();
    await flushMicrotasks();
    // The pane still mounts (inactive, detached) and stays pooled for the next open.
    expect(mounted).toHaveLength(1);
    expect(ready).not.toHaveBeenCalled();
    expect(mounted[0].props.active).toBe(false);
    const again = pool.acquire('s1', context);
    const readyAgain = vi.fn();
    again.ready(readyAgain);
    expect(readyAgain).toHaveBeenCalledWith(mounted[0]);
  });
});

describe('conflicts and ordering', () => {
  it('gives a transient lease for a session already on screen, unmounted on release', async () => {
    const first = await adopt('s1');
    const second = await adopt('s1');
    expect(second.lease.transient).toBe(true);
    expect(second.pane).not.toBe(first.pane);
    expect(second.lease.host).not.toBe(first.lease.host);
    expect(mounted).toHaveLength(2);
    expect(pool.pooledSessionIds()).toEqual(['s1']);
    second.lease.release();
    expect(second.pane!.park).toHaveBeenCalledTimes(1);
    expect((second.lease.host as unknown as FakeHost).parentNode).toBeNull();
    runIdle();
    expect(unmounted).toEqual([second.pane]);
    // The pooled pane is untouched by the transient one going away.
    first.lease.release();
    const hit = pool.acquire('s1', context);
    const ready = vi.fn();
    hit.ready(ready);
    expect(ready).toHaveBeenCalledWith(first.pane);
  });

  it('reuses the entry when a release and an acquire of the same id happen in one tick', async () => {
    // ReferenceModalHost's if/else swap and SessionModal → "View note" → NoteModal.
    const first = await adopt('s1');
    first.lease.release();
    const lease = pool.acquire('s1', context);
    expect(lease.transient).toBe(false);
    const ready = vi.fn();
    lease.ready(ready);
    expect(ready).toHaveBeenCalledWith(first.pane);
    await flushMicrotasks();
    expect(mounted).toHaveLength(1);
  });

  it('falls back to a transient lease when the acquire comes before the release', async () => {
    const first = await adopt('s1');
    const second = await adopt('s1');
    expect(second.lease.transient).toBe(true);
    expect(() => first.lease.release()).not.toThrow();
    expect(pool.pooledSessionIds()).toEqual(['s1']);
    second.lease.release();
    runIdle();
    expect(unmounted).toEqual([second.pane]);
    // The pooled pane is free again and serves the next open.
    const hit = pool.acquire('s1', context);
    const ready = vi.fn();
    hit.ready(ready);
    expect(ready).toHaveBeenCalledWith(first.pane);
  });
});

describe('evictAll', () => {
  it('unmounts free panes on idle and leased panes on their release', async () => {
    const free = await adopt('free');
    free.lease.release();
    const held = await adopt('held');
    pool.evictAll();
    expect(pool.pooledSessionIds()).toEqual([]);
    runIdle();
    expect(unmounted).toEqual([free.pane]);
    held.lease.release();
    expect(held.pane!.park).toHaveBeenCalledTimes(1);
    runIdle();
    expect(unmounted).toEqual([free.pane, held.pane]);
    // A later open of the evicted session mounts afresh.
    const again = await adopt('held');
    expect(again.pane).not.toBe(held.pane);
    expect(mounted).toHaveLength(3);
  });

  it('skips a pending mount for an entry evicted before its microtask', async () => {
    const lease: PaneLease = pool.acquire('s1', context);
    lease.release();
    pool.evictAll();
    await flushMicrotasks();
    expect(mounted).toHaveLength(0);
  });
});
