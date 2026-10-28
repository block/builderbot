/**
 * A pool of mounted SessionChatPanes that outlive the dialogs showing them.
 *
 * Every owner unmounts its session dialog outright on close, and bits-ui's
 * `Dialog.Content` unmounts its children while closed, so a pane inside the
 * dialog is destroyed on every close and rebuilt — four requests and a
 * 40–80 ms transcript mount — on every open. Here the pane is mounted
 * imperatively into a pool-owned host that the dialog *adopts*
 * (`PooledSessionChatPane.svelte`): on close the pane is parked, its host
 * detached, and the next open of the same session reattaches the already
 * built transcript and asks for one `poll_session` delta.
 *
 * Entries are keyed by session id and evicted least-recently-used beyond
 * `capacity`. A session shown twice at once (stacked SessionLauncher modals)
 * gets a *transient* pane that is unmounted on release and never enters the
 * pool, which is exactly today's behaviour.
 *
 * Dependencies are injected so the LRU and lease logic are testable without
 * a DOM or a Svelte runtime.
 */
import { mount as svelteMount, unmount as svelteUnmount } from 'svelte';
import type { HashtagClickInfo } from '../references/referenceHistory.svelte';
import type { HashtagItem, ProjectRepo, Session } from '../../types';
import type { LinkedNoteContext } from './noteFreshness';
import type { DisplayRootInput } from './pathDisplayRoots';
import SessionChatPane from './SessionChatPane.svelte';

/** What the pool and the adopter call on a mounted pane. */
export interface SessionPaneExports {
  park(): void;
  unpark(): void;
  close(): void;
  clearSearch(): void;
  performSearch(query: string): void;
  nextMatch(): void;
  previousMatch(): void;
}

/**
 * SessionChatPane's props as a reactive object. `mount()` reads each prop
 * through its getter, so a write here reaches the mounted pane like a prop
 * change from a template parent would. Fields are `$state.raw`: the pane
 * keys caches on the identity of `hashtagItems` and compares `noteInfo` and
 * `repoLabel` by reference, which a deep proxy would break.
 */
export class PaneProps {
  active = $state.raw(false);
  sessionId = $state.raw('');
  repoDir = $state.raw<DisplayRootInput | undefined>(undefined);
  branchId = $state.raw<string | null | undefined>(undefined);
  projectId = $state.raw<string | null | undefined>(undefined);
  repoLabel = $state.raw<Pick<ProjectRepo, 'githubRepo' | 'subpath' | 'headRepo'> | null>(null);
  hashtagItems = $state.raw<HashtagItem[] | undefined>(undefined);
  noteInfo = $state.raw<LinkedNoteContext | null | undefined>(undefined);
  compact = $state.raw(false);
  onOpenSession = $state.raw<((sessionId: string) => void) | undefined>(undefined);
  onHashtagClick = $state.raw<((click: HashtagClickInfo) => void) | undefined>(undefined);
  onSessionChange = $state.raw<((session: Session | null) => void) | undefined>(undefined);
  onSearchStateChange = $state.raw<
    ((state: { matchCount: number; currentIndex: number }) => void) | undefined
  >(undefined);
}

export interface PaneLease {
  /** Pool-owned `display: contents` wrapper; the adopter appends it to its slot. */
  readonly host: HTMLDivElement;
  readonly props: PaneProps;
  /** The session was already leased, so this pane is unmounted on release. */
  readonly transient: boolean;
  /**
   * Run `cb` with the pane: now on a hit, after the microtask mount on a miss.
   * Only the hit path orders `cb` before the adopter's effects; on a miss the
   * adopter's prop sync (including `active`) has already run by the time the
   * pane mounts, so `cb` sees a pane that mounted with those props.
   */
  ready(cb: (pane: SessionPaneExports) => void): void;
  release(): void;
}

export interface SessionPanePoolDeps {
  mount(host: HTMLDivElement, props: PaneProps, context: Map<unknown, unknown>): SessionPaneExports;
  unmount(pane: SessionPaneExports): void;
  /** Run `cb` off the paint path (an eviction's unmount). */
  scheduleIdle(cb: () => void): void;
  createHost?(): HTMLDivElement;
  capacity?: number;
}

interface Entry {
  readonly sessionId: string;
  readonly host: HTMLDivElement;
  readonly props: PaneProps;
  pane: SessionPaneExports | null;
  /** Callbacks waiting on the microtask mount, owned by the current lease. */
  readyCallbacks: Array<(pane: SessionPaneExports) => void>;
  leased: boolean;
  lastUsed: number;
  transient: boolean;
  /** Left the pool while leased (`evictAll`); unmount on release. */
  evicted: boolean;
  /** Unmounted or about to be; a pending mount must not create a pane. */
  disposed: boolean;
}

export function createSessionPanePool(deps: SessionPanePoolDeps) {
  const capacity = deps.capacity ?? 4;
  const createHost =
    deps.createHost ??
    (() => {
      const host = document.createElement('div');
      host.className = 'session-pane-host';
      // The pane stays the dialog's or aside's flex child; the host adds no box.
      host.style.display = 'contents';
      return host;
    });
  const entries = new Map<string, Entry>();
  let clock = 0;

  function createEntry(sessionId: string, transient: boolean): Entry {
    return {
      sessionId,
      host: createHost(),
      props: new PaneProps(),
      pane: null,
      readyCallbacks: [],
      leased: true,
      lastUsed: ++clock,
      transient,
      evicted: false,
      disposed: false,
    };
  }

  /**
   * Mount from a microtask, never from the caller's effect: a root created
   * while an effect is running is linked under it and destroyed with it, so a
   * pane mounted synchronously from the adopter's attachment would die with
   * its first dialog. By the time the microtask runs the adopter has appended
   * the host, so the pane mounts straight into the document.
   */
  function mountLater(entry: Entry, context: Map<unknown, unknown>) {
    queueMicrotask(() => {
      if (entry.disposed) return;
      const pane = deps.mount(entry.host, entry.props, context);
      entry.pane = pane;
      const callbacks = entry.readyCallbacks;
      entry.readyCallbacks = [];
      for (const cb of callbacks) cb(pane);
    });
  }

  function dispose(entry: Entry) {
    if (entry.disposed) return;
    entry.disposed = true;
    entry.readyCallbacks = [];
    const pane = entry.pane;
    if (!pane) return;
    // The host is already detached, so this is purely effect teardown.
    deps.scheduleIdle(() => deps.unmount(pane));
  }

  function evictBeyondCapacity() {
    while (entries.size > capacity) {
      let oldest: Entry | null = null;
      for (const entry of entries.values()) {
        if (!entry.leased && (oldest === null || entry.lastUsed < oldest.lastUsed)) oldest = entry;
      }
      if (oldest === null) return;
      entries.delete(oldest.sessionId);
      dispose(oldest);
    }
  }

  function lease(entry: Entry): PaneLease {
    let released = false;
    return {
      host: entry.host,
      props: entry.props,
      transient: entry.transient,
      ready(cb) {
        if (released) return;
        if (entry.pane) cb(entry.pane);
        else entry.readyCallbacks.push(cb);
      },
      release() {
        if (released) return;
        released = true;
        // A callback from this lease must not run against the next one.
        entry.readyCallbacks = [];
        // Park before the host leaves its slot; the pane reads what it can of
        // the live box and falls back to its own scroll bookkeeping.
        entry.pane?.park();
        // A parked pane must never call into a destroyed dialog or keep it alive.
        entry.props.onSessionChange = undefined;
        entry.props.onSearchStateChange = undefined;
        entry.props.onOpenSession = undefined;
        entry.props.onHashtagClick = undefined;
        entry.props.active = false;
        // Otherwise `host.parentNode` keeps the closed dialog's whole tree alive.
        entry.host.remove();
        if (entry.transient || entry.evicted) {
          dispose(entry);
          return;
        }
        entry.leased = false;
        entry.lastUsed = ++clock;
      },
    };
  }

  return {
    /**
     * Lease the pane for `sessionId`: the pooled one when it is free, a fresh
     * pooled one when there is none, or a transient one when the pooled pane
     * is already on screen.
     */
    acquire(sessionId: string, context: Map<unknown, unknown>): PaneLease {
      const existing = entries.get(sessionId);
      if (existing && !existing.leased) {
        existing.leased = true;
        existing.lastUsed = ++clock;
        return lease(existing);
      }
      if (existing) {
        const transient = createEntry(sessionId, true);
        mountLater(transient, context);
        return lease(transient);
      }
      const entry = createEntry(sessionId, false);
      entries.set(sessionId, entry);
      mountLater(entry, context);
      evictBeyondCapacity();
      return lease(entry);
    },

    /**
     * Drop every entry. Free panes are unmounted now (off the paint path);
     * leased ones leave the pool and are unmounted when released, so a stale
     * component version never serves another open.
     */
    evictAll(): void {
      for (const entry of entries.values()) {
        if (entry.leased) entry.evicted = true;
        else dispose(entry);
      }
      entries.clear();
    },

    /** Pooled session ids, most recently used last. For tests. */
    pooledSessionIds(): string[] {
      return [...entries.values()].sort((a, b) => a.lastUsed - b.lastUsed).map((e) => e.sessionId);
    },
  };
}

export type SessionPanePool = ReturnType<typeof createSessionPanePool>;

/**
 * `requestIdleCallback` where it exists; WKWebView has none, so fall back to
 * the frame after next plus a task, bounded in case the window is hidden and
 * rAF never fires.
 */
function scheduleIdle(cb: () => void): void {
  let done = false;
  const run = () => {
    if (done) return;
    done = true;
    cb();
  };
  if (typeof requestIdleCallback === 'function') {
    requestIdleCallback(run, { timeout: 1000 });
  } else {
    requestAnimationFrame(() => setTimeout(run, 0));
    setTimeout(run, 1000);
  }
}

export const sessionPanePool: SessionPanePool = createSessionPanePool({
  mount: (target, props, context) => svelteMount(SessionChatPane, { target, props, context }),
  unmount: (pane) => void svelteUnmount(pane),
  scheduleIdle,
});

if (import.meta.hot) {
  // Dev only: a hot update must not keep serving panes built from the old
  // component version.
  import.meta.hot.dispose(() => sessionPanePool.evictAll());
}
