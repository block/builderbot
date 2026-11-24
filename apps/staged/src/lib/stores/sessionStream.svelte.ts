/**
 * The session stream's owner: one per window (or browser tab), since each is
 * its own JavaScript context.
 *
 * The backend pushes session updates to this page over one ordered stream
 * (`session_stream.rs`). The owner is the only thing that talks to it:
 *
 * - **Consumers say what they show.** A component takes a consumer
 *   (`consume`) and gives it the sessions it shows a subtitle for, the ones it
 *   shows only a title for, and the display roots its tool paths are shortened
 *   against. The owner unions every consumer's sessions, each at the highest
 *   level anyone wants, and declares that to the backend once per burst of
 *   changes.
 * - **A claim lingers.** What a consumer stops showing stays declared for
 *   `LINGER_MS`, state included, so a row that scrolls out and back, or a
 *   view that is covered and uncovered, finds its subtitle where it left it.
 * - **State per session, a hint per root set.** A session's status and title
 *   are one value each. Its subtitle depends on the display roots, and two
 *   components can show one session against different roots, so a hint is
 *   kept per session and root set. Each of these is its own reactive value
 *   (`status`, `title`, `hint`), assigned only when it changes: a row reading
 *   one re-renders for its own session's changes and nobody else's.
 * - **Frames.** A snapshot replaces what is held for its session; a delta
 *   replaces the session fields and upserts its rows by id. The rows are
 *   trimmed to the newest `HINT_ROW_WINDOW`, which is also the window the
 *   backend is asked for. The subtitle is derived here, by `deriveHint`: the
 *   backend sends rows as data and processes no text.
 * - **Starting over.** A frame from another generation is dropped. A frame
 *   out of sequence means one was lost, and the owner declares its interest
 *   again under a new generation, which brings snapshots. It does the same
 *   when the stream reopens (every socket connect in the web build, where the
 *   backend forgets a client's interest with its socket), when an interest
 *   call fails, and when the backend's store is replaced (a snapshot that
 *   fell due while it had none is not sent later).
 * - **A hidden window is paused.** The interest is kept and nothing is sent;
 *   showing the window again brings snapshots.
 *
 * `availability` says whether consumers can rely on the stream. It is
 * `unavailable` until `start`, with the kill switch off, and once the stream
 * has gone `ESTABLISH_GRACE_MS` without being established; `liveSessionHints`
 * polls in that state. In between it is `connecting`, where what is shown
 * stays and nothing polls, so a dropped socket does not make every row reload
 * its transcript.
 *
 * Only the title and summary levels are asked for. The full level, the
 * stream health check and a second instance writing the same database are
 * later phases.
 */

import { untrack } from 'svelte';
import { SvelteMap } from 'svelte/reactivity';
import {
  displayRootKey,
  normalizeDisplayRoots,
  resolveDisplayRoots,
  type DisplayRootInput,
} from '../features/sessions/pathDisplayRoots';
import {
  deriveHint,
  derivePipelineHint,
  HINT_ROW_WINDOW,
  normalizeSessionTitle,
} from '../features/timeline/sessionHint';
import {
  listenToEvent,
  openSessionStream,
  type SessionStreamHandlers,
  type SessionStreamLink,
} from '../transport';
import type {
  PipelineExecution,
  SessionStatus,
  SessionStreamFrame,
  SessionStreamInterest,
  SessionStreamLevel,
  SessionSummaryRow,
} from '../types';

// ── Kill switch ─────────────────────────────────────────────────────────────

/**
 * Whether the titles and subtitles of running sessions come from the session
 * stream. `false` puts them back on polling (`liveSessionHints.ts`), which is
 * kept whole as the fallback.
 */
const SESSION_STREAM_ENABLED = true;

/**
 * `localStorage` key that overrides `SESSION_STREAM_ENABLED` for this profile,
 * so the two paths can be compared without a rebuild: `'off'` or `'on'`. Read
 * once when the page starts, so reload after changing it:
 *
 *     localStorage.setItem('staged:session-stream', 'off')
 */
export const SESSION_STREAM_OVERRIDE_KEY = 'staged:session-stream';

export function sessionStreamEnabled(): boolean {
  try {
    const override =
      typeof localStorage === 'undefined'
        ? null
        : localStorage.getItem(SESSION_STREAM_OVERRIDE_KEY);
    if (override === 'on') return true;
    if (override === 'off') return false;
  } catch {
    // No storage to read (a private window can refuse): the constant decides.
  }
  return SESSION_STREAM_ENABLED;
}

// ── Timing ──────────────────────────────────────────────────────────────────

/** How long a claim nobody holds any more stays declared, with its state. */
export const LINGER_MS = 30_000;
/** How long the stream may go unestablished before consumers fall back to polling. */
export const ESTABLISH_GRACE_MS = 5_000;
/** How long changes to the interest gather before one call declares them. */
export const INTEREST_DELAY_MS = 10;
/** The wait after a failed interest call, doubled for each one in a row. */
export const RETRY_BASE_MS = 1_000;
export const RETRY_MAX_MS = 30_000;
/** The least time between two restarts for a lost frame. */
export const GAP_RESTART_SPACING_MS = 1_000;

// ── Types ───────────────────────────────────────────────────────────────────

export type SessionStreamAvailability = 'unavailable' | 'connecting' | 'established';

/** What a consumer shows. Each `sync` replaces the one before. */
export interface SessionStreamWant {
  /** Sessions it shows a subtitle for, and a title. */
  summary: readonly string[];
  /** Sessions it shows a title for and nothing else. */
  title: readonly string[];
  /** The roots its tool paths are shortened against, for the subtitles. */
  displayRoots?: DisplayRootInput;
}

/** One session as a consumer shows it. */
export interface SessionStreamState {
  status: SessionStatus;
  /** The ACP title, cleaned up for display. */
  title: string | undefined;
  /** The subtitle against this consumer's display roots. Summary sessions only. */
  hint: string | undefined;
}

export interface SessionStreamConsumer {
  sync(want: SessionStreamWant): void;
  /**
   * A session this consumer asked for, or `undefined` while nothing has
   * arrived for it at the level it asked for. Not a reactive read.
   */
  read(sessionId: string): SessionStreamState | undefined;
  /** Stop showing everything. The consumer is not used again. */
  release(): void;
}

/** What the owner needs from outside, so tests can stand in for it. */
export interface SessionStreamDeps {
  open: (handlers: SessionStreamHandlers) => SessionStreamLink;
  resolveRoots: (roots: DisplayRootInput) => Promise<string[]>;
  /** Calls `listener` when the backend's store has been replaced. */
  onStoreReset: (listener: () => void) => () => void;
  /** The page's visibility, or `null` where there is no page. */
  visibility: {
    hidden: () => boolean;
    listen: (listener: () => void) => () => void;
  } | null;
}

type Claim = {
  sessionId: string;
  level: SessionStreamLevel;
  /** `displayRootKey` of the consumer's roots. Empty for a title claim. */
  rootsKey: string;
  roots: DisplayRootInput;
};

type ConsumerState = {
  onChange: ((sessionId: string) => void) | undefined;
  claims: Map<string, Claim>;
  /** The key of the roots given with the last `sync`. */
  rootsKey: string;
};

/** A subtitle of one session against one consumer root set. */
type View = {
  /** The consumer's roots. The session's working directory joins them. */
  roots: DisplayRootInput;
  /** `displayRootKey` of the two together, once worked out. */
  rootKey: string | null;
  displayRoots: string[];
  /** False while the aliases of its roots are being resolved. */
  ready: boolean;
};

type Entry = {
  /** The highest level a claim asks for. */
  level: SessionStreamLevel;
  /** The level the backend was last told. */
  declared: SessionStreamLevel | null;
  /** A frame has been applied. */
  loaded: boolean;
  /** A summary-level frame has been applied since the rows were last dropped. */
  summarized: boolean;
  workingDir: string | undefined;
  pipeline: PipelineExecution | undefined;
  openingPromptId: number | null;
  /** The newest rows, in id order. */
  rows: SessionSummaryRow[];
  /** By the consumer roots' key. */
  views: Map<string, View>;
};

function claimKey(level: SessionStreamLevel, sessionId: string, rootsKey: string): string {
  return `${level}\n${sessionId}\n${rootsKey}`;
}

function hintKey(sessionId: string, rootsKey: string): string {
  return `${sessionId}\n${rootsKey}`;
}

/**
 * A reactive value per key. `undefined` is a value like any other: a cell is
 * made when its session (or view) is taken up and deleted only when that is
 * dropped, and in between an empty title or subtitle is a cell holding
 * `undefined`.
 *
 * That is what keeps a read to its own session. `SvelteMap` gives each key
 * that exists a signal of its own, and every read of a key that does not
 * exist the same one, which fires whenever any key is added, removed or
 * changed. So a row whose title was deleted when it became empty would be
 * evaluated again for every other session's change.
 */
type Cells<V> = SvelteMap<string, V | undefined>;

/** Assign a cell only when its value changes. */
function setCell<V>(cells: Cells<V>, key: string, value: V | undefined): void {
  if (cells.get(key) !== value) cells.set(key, value);
}

/** The current version of each updated row over the ones held, newest window kept. */
function upsertRows(
  rows: SessionSummaryRow[],
  updates: readonly SessionSummaryRow[]
): SessionSummaryRow[] {
  if (updates.length === 0) return rows;
  const byId = new Map(rows.map((row) => [row.id, row]));
  for (const row of updates) byId.set(row.id, row);
  return [...byId.values()].sort((a, b) => a.id - b.id).slice(-HINT_ROW_WINDOW);
}

function documentVisibility(): SessionStreamDeps['visibility'] {
  if (typeof document === 'undefined') return null;
  return {
    hidden: () => document.hidden,
    listen: (listener) => {
      document.addEventListener('visibilitychange', listener);
      return () => document.removeEventListener('visibilitychange', listener);
    },
  };
}

const defaultDeps: SessionStreamDeps = {
  open: openSessionStream,
  resolveRoots: (roots) => resolveDisplayRoots(roots),
  onStoreReset: (listener) => listenToEvent<void>('store-reset-completed', listener),
  visibility: documentVisibility(),
};

// ── Owner ───────────────────────────────────────────────────────────────────

export class SessionStreamOwner {
  readonly #deps: SessionStreamDeps;

  // The reactive values. Everything else here is plain state.
  readonly #statuses: Cells<SessionStatus> = new SvelteMap();
  readonly #titles: Cells<string> = new SvelteMap();
  /** By `hintKey`. */
  readonly #hints: Cells<string> = new SvelteMap();

  readonly #entries = new Map<string, Entry>();
  readonly #consumers = new Set<ConsumerState>();
  /** Claims nobody holds any more, by `claimKey`, until their timer drops them. */
  readonly #lingering = new Map<string, { claim: Claim; timer: ReturnType<typeof setTimeout> }>();

  #link: SessionStreamLink | null = null;
  #stopListening: (() => void)[] = [];
  /** The link can take an interest. */
  #open = false;
  #paused = false;
  #generation = 0;
  /** The sequence number of the last frame applied in this generation. */
  #lastSeq = 0;
  /** The last interest the backend accepted, serialized. */
  #sentKey: string | null = null;
  #sending = false;
  #sendTimer: ReturnType<typeof setTimeout> | null = null;
  /** When `#sendTimer` fires. */
  #sendDue = 0;
  /** No interest call before this time: the wait after a failure or a lost frame. */
  #notBefore = 0;
  #failures = 0;
  #lastGapRestartAt = 0;

  #availability: SessionStreamAvailability = 'unavailable';
  #graceTimer: ReturnType<typeof setTimeout> | null = null;
  readonly #availabilityListeners = new Set<(availability: SessionStreamAvailability) => void>();

  constructor(deps: Partial<SessionStreamDeps> = {}) {
    this.#deps = { ...defaultDeps, ...deps };
  }

  // ── Lifecycle ─────────────────────────────────────────────────────────────

  /** Open the stream. Until this is called the owner is `unavailable`. */
  start(): void {
    if (this.#link) return;
    const visibility = this.#deps.visibility;
    this.#paused = visibility?.hidden() ?? false;
    if (visibility) {
      this.#stopListening.push(
        visibility.listen(() => {
          const paused = visibility.hidden();
          if (this.#paused === paused) return;
          this.#paused = paused;
          this.#requestSend();
        })
      );
    }
    // A snapshot that fell due while the backend had no store was not sent
    // and will not be, so ask again once it has one.
    this.#stopListening.push(this.#deps.onStoreReset(() => this.#restart()));
    this.#setAvailability('connecting');
    this.#link = this.#deps.open({
      onFrame: (frame) => this.#receive(frame),
      onOpen: () => {
        // The backend holds nothing for this stream, so the whole interest
        // is declared, an empty one included: its answer is what shows the
        // stream is there.
        this.#open = true;
        this.#failures = 0;
        this.#notBefore = 0;
        this.#restart();
      },
      onClose: () => {
        this.#open = false;
        this.#clearSendTimer();
        this.#lost();
      },
    });
  }

  /** Close the stream and forget what it brought. Consumers keep their claims. */
  stop(): void {
    const link = this.#link;
    if (!link) return;
    this.#link = null;
    for (const stop of this.#stopListening.splice(0)) stop();
    this.#clearSendTimer();
    if (this.#graceTimer !== null) clearTimeout(this.#graceTimer);
    this.#graceTimer = null;
    if (this.#open) {
      // The backend keeps a client's interest for as long as its window or
      // socket lives, so hand it an empty one on the way out.
      this.#generation += 1;
      void link
        .setInterest({ ...this.#interest(), sessions: [] })
        .catch(() => {})
        .finally(() => link.close());
    } else {
      link.close();
    }
    this.#open = false;
    this.#sentKey = null;
    this.#lastSeq = 0;
    for (const [sessionId, entry] of this.#entries) {
      entry.declared = null;
      this.#forget(sessionId, entry);
    }
    this.#setAvailability('unavailable');
  }

  get availability(): SessionStreamAvailability {
    return this.#availability;
  }

  /** Call `listener` whenever `availability` changes. */
  onAvailability(listener: (availability: SessionStreamAvailability) => void): () => void {
    this.#availabilityListeners.add(listener);
    return () => this.#availabilityListeners.delete(listener);
  }

  // ── Reactive reads ────────────────────────────────────────────────────────

  // Each of these is one session's own value: reading it subscribes to that
  // session and no other, from the moment a consumer has asked for it. A
  // read made before that waits on the owner taking the session up.

  /** A session's status, once a frame has brought it. */
  status(sessionId: string): SessionStatus | undefined {
    return this.#statuses.get(sessionId);
  }

  /** A session's ACP title, cleaned up for display. */
  title(sessionId: string): string | undefined {
    return this.#titles.get(sessionId);
  }

  /**
   * A session's subtitle against `displayRoots`, which have to be roots a
   * consumer gave when it asked for the session's summary.
   */
  hint(sessionId: string, displayRoots?: DisplayRootInput): string | undefined {
    return this.#hints.get(hintKey(sessionId, displayRootKey(displayRoots)));
  }

  // ── Consumers ─────────────────────────────────────────────────────────────

  /**
   * Take a consumer. `onChange` is called with a session's id when something
   * the consumer asked for about it may have changed, for callers that are
   * not reading the reactive values.
   */
  consume(onChange?: (sessionId: string) => void): SessionStreamConsumer {
    const state: ConsumerState = { onChange, claims: new Map(), rootsKey: '' };
    this.#consumers.add(state);
    let released = false;

    // The owner's own reads must not subscribe the caller: a consumer is
    // synced from a component's effect, which would otherwise re-run for
    // every value read here.
    const sync = (want: SessionStreamWant) =>
      untrack(() => {
        if (released) return;
        const rootsKey = displayRootKey(want.displayRoots);
        const claims = new Map<string, Claim>();
        const summary = new Set(want.summary);
        for (const sessionId of summary) {
          claims.set(claimKey('summary', sessionId, rootsKey), {
            sessionId,
            level: 'summary',
            rootsKey,
            roots: want.displayRoots,
          });
        }
        for (const sessionId of want.title) {
          if (summary.has(sessionId)) continue;
          claims.set(claimKey('title', sessionId, ''), {
            sessionId,
            level: 'title',
            rootsKey: '',
            roots: undefined,
          });
        }
        for (const [key, claim] of state.claims) {
          if (!claims.has(key)) this.#linger(key, claim);
        }
        state.claims = claims;
        state.rootsKey = rootsKey;
        this.#reconcile();
      });

    return {
      sync,
      read: (sessionId) => untrack(() => this.#read(state, sessionId)),
      release: () => {
        if (released) return;
        sync({ summary: [], title: [] });
        released = true;
        this.#consumers.delete(state);
      },
    };
  }

  #read(consumer: ConsumerState, sessionId: string): SessionStreamState | undefined {
    const entry = this.#entries.get(sessionId);
    const status = this.#statuses.get(sessionId);
    if (!entry?.loaded || status === undefined) return undefined;
    const title = this.#titles.get(sessionId);
    if (!consumer.claims.has(claimKey('summary', sessionId, consumer.rootsKey))) {
      return { status, title, hint: undefined };
    }
    if (!entry.summarized || !entry.views.get(consumer.rootsKey)?.ready) return undefined;
    return { status, title, hint: this.#hints.get(hintKey(sessionId, consumer.rootsKey)) };
  }

  #notify(sessionId: string): void {
    for (const consumer of [...this.#consumers]) {
      if (!consumer.onChange) continue;
      if (
        consumer.claims.has(claimKey('summary', sessionId, consumer.rootsKey)) ||
        consumer.claims.has(claimKey('title', sessionId, ''))
      ) {
        consumer.onChange(sessionId);
      }
    }
  }

  /** Keep a claim its consumer let go of for `LINGER_MS`. */
  #linger(key: string, claim: Claim): void {
    const lingering = this.#lingering.get(key);
    if (lingering) clearTimeout(lingering.timer);
    const timer = setTimeout(() => {
      this.#lingering.delete(key);
      this.#reconcile();
    }, LINGER_MS);
    this.#lingering.set(key, { claim, timer });
  }

  /** Bring the entries in line with the claims, held and lingering. */
  #reconcile(): void {
    const wanted = new Map<
      string,
      { level: SessionStreamLevel; views: Map<string, DisplayRootInput> }
    >();
    const want = (claim: Claim) => {
      let session = wanted.get(claim.sessionId);
      if (!session) {
        session = { level: 'title', views: new Map() };
        wanted.set(claim.sessionId, session);
      }
      if (claim.level === 'summary') {
        session.level = 'summary';
        session.views.set(claim.rootsKey, claim.roots);
      }
    };
    for (const consumer of this.#consumers) consumer.claims.forEach(want);
    for (const { claim } of this.#lingering.values()) want(claim);

    for (const [sessionId, entry] of this.#entries) {
      if (wanted.has(sessionId)) continue;
      this.#entries.delete(sessionId);
      this.#statuses.delete(sessionId);
      this.#titles.delete(sessionId);
      for (const rootsKey of entry.views.keys()) this.#hints.delete(hintKey(sessionId, rootsKey));
    }
    for (const [sessionId, { level, views }] of wanted) {
      let entry = this.#entries.get(sessionId);
      if (!entry) {
        entry = {
          level,
          declared: null,
          loaded: false,
          summarized: false,
          workingDir: undefined,
          pipeline: undefined,
          openingPromptId: null,
          rows: [],
          views: new Map(),
        };
        this.#entries.set(sessionId, entry);
        this.#statuses.set(sessionId, undefined);
        this.#titles.set(sessionId, undefined);
      }
      entry.level = level;
      for (const rootsKey of [...entry.views.keys()]) {
        if (views.has(rootsKey)) continue;
        entry.views.delete(rootsKey);
        this.#hints.delete(hintKey(sessionId, rootsKey));
      }
      for (const [rootsKey, roots] of views) {
        if (entry.views.has(rootsKey)) continue;
        entry.views.set(rootsKey, { roots, rootKey: null, displayRoots: [], ready: false });
        this.#hints.set(hintKey(sessionId, rootsKey), undefined);
        this.#refreshView(sessionId, entry, rootsKey);
      }
    }
    this.#requestSend();
  }

  /** Drop what the frames brought for a session that is still held. */
  #forget(sessionId: string, entry: Entry): void {
    setCell(this.#statuses, sessionId, undefined);
    setCell(this.#titles, sessionId, undefined);
    entry.loaded = false;
    this.#forgetSummary(sessionId, entry);
  }

  #forgetSummary(sessionId: string, entry: Entry): void {
    entry.summarized = false;
    entry.workingDir = undefined;
    entry.pipeline = undefined;
    entry.openingPromptId = null;
    entry.rows = [];
    for (const rootsKey of entry.views.keys()) {
      setCell(this.#hints, hintKey(sessionId, rootsKey), undefined);
    }
  }

  // ── Hints ─────────────────────────────────────────────────────────────────

  /**
   * Derive one view's subtitle from the rows held. The first time, and when
   * the session's working directory changes, the aliases of its roots are
   * resolved first, as the poller does, so a tool path is not shown long and
   * then shortened.
   */
  #refreshView(sessionId: string, entry: Entry, rootsKey: string): void {
    const view = entry.views.get(rootsKey);
    if (!view || !entry.summarized) return;
    const candidates: DisplayRootInput = [view.roots, entry.workingDir];
    const rootKey = displayRootKey(candidates);
    if (view.rootKey !== rootKey) {
      view.rootKey = rootKey;
      view.displayRoots = normalizeDisplayRoots(candidates);
      view.ready = !rootKey;
      if (rootKey) {
        const settle = (resolved: string[] | null) => {
          const current =
            this.#entries.get(sessionId) === entry && entry.views.get(rootsKey) === view;
          if (!current || view.rootKey !== rootKey) return;
          if (resolved) view.displayRoots = resolved;
          view.ready = true;
          this.#refreshView(sessionId, entry, rootsKey);
          this.#notify(sessionId);
        };
        void this.#deps.resolveRoots(candidates).then(settle, () => settle(null));
      }
    }
    if (!view.ready) return;
    const hint =
      deriveHint(entry.rows, view.displayRoots, entry.openingPromptId) ??
      (entry.rows.length === 0 ? derivePipelineHint(entry.pipeline) : undefined);
    setCell(this.#hints, hintKey(sessionId, rootsKey), hint);
  }

  // ── Frames ────────────────────────────────────────────────────────────────

  #receive(frame: SessionStreamFrame): void {
    // An answer to an interest that has since been replaced.
    if (!this.#link || frame.generation !== this.#generation) return;
    if (frame.seq !== this.#lastSeq + 1) {
      // A frame was lost, and with it whatever it carried. Ask again; this
      // frame goes with its generation.
      const now = Date.now();
      this.#notBefore = Math.max(this.#notBefore, this.#lastGapRestartAt + GAP_RESTART_SPACING_MS);
      this.#lastGapRestartAt = now;
      this.#restart();
      return;
    }
    this.#lastSeq = frame.seq;
    this.#apply(frame);
  }

  #apply(frame: SessionStreamFrame): void {
    const sessionId = frame.sessionId;
    const entry = this.#entries.get(sessionId);
    const summary = frame.summary;
    if (!entry || !summary) return;

    entry.loaded = true;
    setCell(this.#statuses, sessionId, summary.status);
    setCell(this.#titles, sessionId, normalizeSessionTitle(summary.acpTitle));

    // A title-level frame carries no working directory, and its other
    // summary fields are empty, not current. One can still be on its way
    // after the session was raised to the summary level.
    if (entry.declared === 'summary' && summary.workingDir !== undefined) {
      entry.workingDir = summary.workingDir;
      entry.pipeline = summary.pipeline;
      if (frame.kind === 'snapshot') {
        entry.rows = summary.rows.slice(-HINT_ROW_WINDOW);
        entry.openingPromptId = summary.openingPromptId;
      } else {
        entry.rows = upsertRows(entry.rows, summary.rows);
        // The first user row, so once known it does not change.
        entry.openingPromptId = summary.openingPromptId ?? entry.openingPromptId;
      }
      entry.summarized = true;
      for (const rootsKey of entry.views.keys()) this.#refreshView(sessionId, entry, rootsKey);
    }
    this.#notify(sessionId);
  }

  // ── Interest ──────────────────────────────────────────────────────────────

  #interest(): SessionStreamInterest {
    const sessions = [...this.#entries]
      .map(([sessionId, entry]) => ({ sessionId, level: entry.level }))
      .sort((a, b) => (a.sessionId < b.sessionId ? -1 : a.sessionId > b.sessionId ? 1 : 0));
    return {
      generation: this.#generation,
      paused: this.#paused,
      summaryWindow: HINT_ROW_WINDOW,
      sessions,
    };
  }

  /** Start over under a new generation: every session gets a snapshot. */
  #restart(): void {
    this.#generation += 1;
    this.#lastSeq = 0;
    this.#requestSend();
  }

  #clearSendTimer(): void {
    if (this.#sendTimer !== null) clearTimeout(this.#sendTimer);
    this.#sendTimer = null;
  }

  /** Declare the interest soon, if it differs from what the backend has. */
  #requestSend(): void {
    // Closed: `onOpen` sends. Sending: the call's end looks again.
    if (!this.#link || !this.#open || this.#sending) return;
    if (JSON.stringify(this.#interest()) === this.#sentKey) return;
    const now = Date.now();
    const due = Math.max(now + INTEREST_DELAY_MS, this.#notBefore);
    if (this.#sendTimer !== null) {
      // The burst's first change set the timer, and later ones ride on it,
      // unless a wait has been imposed since that it would cut short.
      if (this.#sendDue >= due) return;
      clearTimeout(this.#sendTimer);
    }
    this.#sendDue = due;
    this.#sendTimer = setTimeout(() => {
      this.#sendTimer = null;
      void this.#send();
    }, due - now);
  }

  async #send(): Promise<void> {
    const link = this.#link;
    if (!link || !this.#open || this.#sending) return;
    const interest = this.#interest();
    const key = JSON.stringify(interest);
    if (key === this.#sentKey) return;

    // From here the backend is being told these levels. A session lowered
    // to its title gets no more rows, so the ones held would go stale.
    for (const [sessionId, entry] of this.#entries) {
      if (entry.declared === 'summary' && entry.level === 'title') {
        this.#forgetSummary(sessionId, entry);
      }
      entry.declared = entry.level;
    }

    this.#sending = true;
    let failed = false;
    try {
      await link.setInterest(interest);
    } catch (e) {
      failed = true;
      console.warn('[sessionStream] The interest was not accepted:', e);
    }
    this.#sending = false;
    if (this.#link !== link || !this.#open) return;

    if (failed) {
      // The backend may hold an older interest or none, so the retry starts
      // over; in Tauri mode it also opens a new stream.
      this.#failures += 1;
      this.#notBefore =
        Date.now() + Math.min(RETRY_BASE_MS * 2 ** (this.#failures - 1), RETRY_MAX_MS);
      this.#lost();
      this.#restart();
      return;
    }
    this.#failures = 0;
    if (interest.generation === this.#generation) {
      this.#sentKey = key;
      this.#setAvailability('established');
    }
    // Whatever changed while the call ran.
    this.#requestSend();
  }

  // ── Availability ──────────────────────────────────────────────────────────

  /** The stream stopped being established. Consumers get a grace period. */
  #lost(): void {
    if (this.#availability === 'established') this.#setAvailability('connecting');
  }

  #setAvailability(next: SessionStreamAvailability): void {
    if (this.#availability === next) return;
    this.#availability = next;
    if (this.#graceTimer !== null) clearTimeout(this.#graceTimer);
    this.#graceTimer = null;
    if (next === 'connecting') {
      this.#graceTimer = setTimeout(() => {
        this.#graceTimer = null;
        this.#setAvailability('unavailable');
      }, ESTABLISH_GRACE_MS);
    }
    for (const listener of [...this.#availabilityListeners]) listener(next);
  }
}

/** This window's owner. */
export const sessionStream = new SessionStreamOwner();

/**
 * Start this window's session stream unless the kill switch is off. Returns
 * the function that stops it.
 */
export function startSessionStream(): () => void {
  if (!sessionStreamEnabled()) return () => {};
  sessionStream.start();
  return () => sessionStream.stop();
}
