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
 * - **Open chats take the frames themselves.** A chat pane opens a chat
 *   (`openChat`) for the session it shows, and the session is declared at the
 *   full level beside whatever level its rows want. The owner keeps nothing
 *   of it: each full-level frame is handed to the pane, which merges it into
 *   its own window with the code it merges a poll with. The pane says what
 *   it holds as a poll's cursors, and the owner gives the backend that
 *   cursor whenever a snapshot is wanted: when a pane joins (two panes can
 *   show one session, and both take the snapshot read for the one that
 *   joined), when the stream starts over or a hidden window is shown, when
 *   the backend asks with a `resync` frame, and when a pane asks. Between
 *   those the cursor is left out and the deltas carry on. A chat does not
 *   linger: a parked pane catches up from its cursor when it is shown again.
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
 * - **A health check.** A lost frame shows as a gap only when a later frame
 *   arrives, so the last frame of a quiet stream can go missing unnoticed.
 *   On the status listener's re-hydrate triggers (a return to the window and
 *   once a minute in a desktop window, a resume or reconnect in the web
 *   build) the owner asks the backend whether its last frame is the last one
 *   applied here, and starts over if not. It only asks while the stream is
 *   established in the generation it last declared and the window is
 *   showing, so it never competes with a start-over already under way.
 *
 * `availability` says whether consumers can rely on the stream. It is
 * `unavailable` until `start`, and once the stream has gone
 * `ESTABLISH_GRACE_MS` without being established: the socket is down, the
 * interest keeps being refused, the hub has stopped. In between it is
 * `connecting`, where what is shown stays, so a dropped socket or one failed
 * call changes nothing a user can see. Nothing else brings a row its title
 * and subtitle, so when the stream is given up on the owner drops what the
 * frames brought, and a row shows its static label and not a subtitle that
 * has stopped moving. It goes on trying, and starts over when the stream is
 * back, which brings snapshots. An open chat is the one thing that polls
 * while the stream is unavailable (`SessionChatPane.svelte`).
 *
 * A second Staged process writing the same database is the backend's to
 * notice (`session_stream/watch.rs`): its changes arrive as deltas like any
 * other at the title and summary levels, and as a `resync` at the full level.
 */

import { untrack } from 'svelte';
import { SvelteMap } from 'svelte/reactivity';
import {
  displayRootKey,
  normalizeDisplayRoots,
  resolveDisplayRoots,
  type DisplayRootInput,
} from '../features/sessions/pathDisplayRoots';
import { unionSessionPollRequests } from '../features/sessions/messagePoll';
import {
  deriveHint,
  derivePipelineHint,
  HINT_ROW_WINDOW,
  normalizeSessionTitle,
} from '../features/timeline/sessionHint';
import { onRehydrate } from '../listeners/rehydrateSignal';
import {
  listenToEvent,
  openSessionStream,
  type SessionStreamHandlers,
  type SessionStreamLink,
} from '../transport';
import type {
  PipelineExecution,
  SessionFull,
  SessionPollRequest,
  SessionStatus,
  SessionStreamFrame,
  SessionStreamInterest,
  SessionStreamLevel,
  SessionStreamSessionInterest,
  SessionSummaryRow,
} from '../types';

// ── Timing ──────────────────────────────────────────────────────────────────

/** How long a claim nobody holds any more stays declared, with its state. */
export const LINGER_MS = 30_000;
/**
 * How long the stream may go unestablished before it counts as unavailable:
 * rows drop what it brought, and an open chat polls.
 */
export const ESTABLISH_GRACE_MS = 5_000;
/** How long changes to the interest gather before one call declares them. */
export const INTEREST_DELAY_MS = 10;
/** The wait after a failed interest call, doubled for each one in a row. */
export const RETRY_BASE_MS = 1_000;
export const RETRY_MAX_MS = 30_000;
/** The least time between two restarts for a lost frame. */
export const GAP_RESTART_SPACING_MS = 1_000;
/**
 * How long a failed health check waits for a frame that may still be on its
 * way before it counts as a lost one.
 */
export const HEALTH_CONFIRM_MS = 1_000;
/**
 * How many times one health check asks. Each answer that frames overtook is
 * asked again; a stream busy enough to overtake them all is delivering, and
 * a frame it loses shows as a gap.
 */
export const HEALTH_ATTEMPTS = 3;

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

/** What an open chat gives the owner. */
export interface SessionChatHandlers {
  /**
   * What the pane holds of the session now, as the cursors of a poll
   * (`sessionPollRequest`). Asked each time a snapshot is to be read.
   */
  cursor: () => SessionPollRequest;
  /**
   * A full-level frame for the session. A snapshot is read from a cursor at
   * least as old as this pane's, and a delta holds what was written; either
   * is merged by id.
   */
  onFrame: (kind: 'snapshot' | 'delta', full: SessionFull) => void;
}

/** One pane's hold on a session at the full level. */
export interface SessionChat {
  /**
   * Ask for a snapshot read from the pane's cursor as it is now: the pane's
   * own re-read of the session, in order with the frames.
   */
  resync(): void;
  /** Stop taking frames. The chat is not used again. */
  release(): void;
}

/** What the owner needs from outside, so tests can stand in for it. */
export interface SessionStreamDeps {
  open: (handlers: SessionStreamHandlers) => SessionStreamLink;
  resolveRoots: (roots: DisplayRootInput) => Promise<string[]>;
  /** Calls `listener` when the backend's store has been replaced. */
  onStoreReset: (listener: () => void) => () => void;
  /** Calls `listener` when the stream's health is due a check. */
  onHealthTrigger: (listener: () => void) => () => void;
  /** The page's visibility, or `null` where there is no page. */
  visibility: {
    hidden: () => boolean;
    listen: (listener: () => void) => () => void;
  } | null;
}

/** The levels a row is shown at. The full level is an open chat's (`Chat`). */
type RowLevel = Exclude<SessionStreamLevel, 'full'>;

type Claim = {
  sessionId: string;
  level: RowLevel;
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
  level: RowLevel;
  /** The level the backend was last told. */
  declared: RowLevel | null;
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

/** A session open as a chat, in one pane or more. */
type Chat = {
  panes: Set<SessionChatHandlers>;
  /** The next interest gives the panes' cursor, which asks for a snapshot. */
  wantsSnapshot: boolean;
};

function claimKey(level: RowLevel, sessionId: string, rootsKey: string): string {
  return `${level}\n${sessionId}\n${rootsKey}`;
}

function order(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
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

/**
 * A subtitle's cell. `undefined` until the subtitle has been derived (no
 * summary frame yet, or its roots are still being resolved), then the
 * subtitle, or `null` for a session with nothing to show.
 */
type HintCells = Cells<string | null>;

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
  onHealthTrigger: onRehydrate,
  visibility: documentVisibility(),
};

// ── Owner ───────────────────────────────────────────────────────────────────

export class SessionStreamOwner {
  readonly #deps: SessionStreamDeps;

  // The reactive values. Everything else here is plain state.
  readonly #statuses: Cells<SessionStatus> = new SvelteMap();
  readonly #titles: Cells<string> = new SvelteMap();
  /** By `hintKey`. */
  readonly #hints: HintCells = new SvelteMap();

  readonly #entries = new Map<string, Entry>();
  /** The sessions open as chats. Held apart from `#entries`: see `Chat`. */
  readonly #chats = new Map<string, Chat>();
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
  /** The generation of that interest: the one the backend numbers frames in. */
  #acceptedGeneration: number | null = null;
  /** A health check is under way. */
  #checking = false;
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
          // The backend reads a chat again when a pause ends, from the last
          // cursor it was given. Give it what the panes hold now.
          if (!paused) this.#wantSnapshots();
          this.#requestSend();
        })
      );
    }
    // A snapshot that fell due while the backend had no store was not sent
    // and will not be, so ask again once it has one.
    this.#stopListening.push(this.#deps.onStoreReset(() => this.#restart()));
    this.#stopListening.push(this.#deps.onHealthTrigger(() => void this.checkHealth()));
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
        this.#acceptedGeneration = null;
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
    this.#acceptedGeneration = null;
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
    return this.#hints.get(hintKey(sessionId, displayRootKey(displayRoots))) ?? undefined;
  }

  /**
   * Whether `hint` is the session's subtitle against `displayRoots` as the
   * frames have it. False while it has not been derived yet, where `hint`
   * is `undefined` for want of rows and not for want of anything to show.
   */
  hintDerived(sessionId: string, displayRoots?: DisplayRootInput): boolean {
    return this.#hints.get(hintKey(sessionId, displayRootKey(displayRoots))) !== undefined;
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

  // ── Chats ─────────────────────────────────────────────────────────────────

  /**
   * Take a session at the full level for a pane that shows it as a chat.
   * The pane is sent a snapshot read from its cursor, then deltas, for as
   * long as the stream is established; it is the pane's to watch
   * `availability` and poll while the stream is unavailable.
   */
  openChat(sessionId: string, handlers: SessionChatHandlers): SessionChat {
    let chat = this.#chats.get(sessionId);
    if (!chat) {
      chat = { panes: new Set(), wantsSnapshot: true };
      this.#chats.set(sessionId, chat);
    }
    const held = chat;
    held.panes.add(handlers);
    // Also for a pane joining one already there: what it holds is its own.
    held.wantsSnapshot = true;
    this.#requestSend();

    let released = false;
    return {
      resync: () => {
        if (released) return;
        held.wantsSnapshot = true;
        this.#requestSend();
      },
      release: () => {
        if (released) return;
        released = true;
        held.panes.delete(handlers);
        if (held.panes.size === 0 && this.#chats.get(sessionId) === held) {
          this.#chats.delete(sessionId);
        }
        this.#requestSend();
      },
    };
  }

  /** Have every chat read again from its panes' cursors. */
  #wantSnapshots(): void {
    for (const chat of this.#chats.values()) chat.wantsSnapshot = true;
  }

  /** Whether a chat is waiting to give its cursor, and the backend would read it. */
  #snapshotWanted(): boolean {
    if (this.#paused) return false;
    for (const chat of this.#chats.values()) {
      if (chat.wantsSnapshot) return true;
    }
    return false;
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
    return {
      status,
      title,
      hint: this.#hints.get(hintKey(sessionId, consumer.rootsKey)) ?? undefined,
    };
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
    const wanted = new Map<string, { level: RowLevel; views: Map<string, DisplayRootInput> }>();
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
   * resolved first, so a tool path is not shown long and then shortened.
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
    setCell(this.#hints, hintKey(sessionId, rootsKey), hint ?? null);
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
    if (frame.kind === 'resync') {
      // The backend sends the chat nothing more until it has the cursor. A
      // chat that has been let go of is dropped by the interest already on
      // its way, which answers as well.
      const chat = this.#chats.get(sessionId);
      if (chat) {
        chat.wantsSnapshot = true;
        this.#requestSend();
      }
      return;
    }
    if (frame.full) {
      // A frame is about one level, so this one has nothing for the rows.
      const chat = this.#chats.get(sessionId);
      if (chat) for (const pane of [...chat.panes]) pane.onFrame(frame.kind, frame.full);
      return;
    }
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

  /**
   * What is wanted, without the chats' cursors: they say what the panes hold
   * and not what is wanted, and are added by the call that gives them.
   */
  #interest(): SessionStreamInterest {
    const sessions: SessionStreamSessionInterest[] = [
      ...[...this.#entries].map(([sessionId, entry]) => ({ sessionId, level: entry.level })),
      ...[...this.#chats.keys()].map((sessionId) => ({ sessionId, level: 'full' as const })),
    ].sort((a, b) => order(a.sessionId, b.sessionId) || order(a.level, b.level));
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
    this.#wantSnapshots();
    this.#requestSend();
  }

  #clearSendTimer(): void {
    if (this.#sendTimer !== null) clearTimeout(this.#sendTimer);
    this.#sendTimer = null;
  }

  /**
   * Declare the interest soon, if it differs from what the backend has or a
   * chat has a cursor to give.
   */
  #requestSend(): void {
    // Closed: `onOpen` sends. Sending: the call's end looks again.
    if (!this.#link || !this.#open || this.#sending) return;
    if (JSON.stringify(this.#interest()) === this.#sentKey && !this.#snapshotWanted()) return;
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
    if (key === this.#sentKey && !this.#snapshotWanted()) return;

    // The cursors are read now, as late as can be: a frame applied after
    // this only puts a pane ahead of its cursor, and the snapshot then
    // repeats rows the pane holds. A paused backend reads nothing, so they
    // wait for the call that ends the pause.
    if (!this.#paused) {
      for (const session of interest.sessions) {
        const chat = session.level === 'full' ? this.#chats.get(session.sessionId) : undefined;
        if (!chat?.wantsSnapshot) continue;
        session.cursor = unionSessionPollRequests([...chat.panes].map((pane) => pane.cursor()));
        chat.wantsSnapshot = false;
      }
    }

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
      this.#acceptedGeneration = null;
      this.#notBefore =
        Date.now() + Math.min(RETRY_BASE_MS * 2 ** (this.#failures - 1), RETRY_MAX_MS);
      this.#lost();
      this.#restart();
      return;
    }
    this.#failures = 0;
    // The backend numbers its frames in this generation now, whether or not
    // a newer one has been started here since.
    this.#acceptedGeneration = interest.generation;
    if (interest.generation === this.#generation) {
      this.#sentKey = key;
      this.#setAvailability('established');
    }
    // Whatever changed while the call ran.
    this.#requestSend();
  }

  // ── Health ────────────────────────────────────────────────────────────────

  /**
   * Whether the stream is in a state a health check can say anything about:
   * established, showing, and in the generation the backend last accepted.
   * Otherwise a start-over is already on its way, or nothing is being sent.
   */
  #checkable(link: SessionStreamLink, generation: number): boolean {
    return (
      this.#link === link &&
      this.#open &&
      !this.#paused &&
      this.#availability === 'established' &&
      this.#generation === generation &&
      this.#acceptedGeneration === generation
    );
  }

  /**
   * Ask the backend whether the last frame it sent is the last one applied
   * here, and start over under a new generation if it is not: a frame was
   * lost with none after it to show the gap, or the backend no longer holds
   * this stream.
   *
   * The backend's answer is "no" for a frame that is still on its way too.
   * So a "no" is only believed once nothing has arrived for
   * `HEALTH_CONFIRM_MS`, counted from before the question was asked. If
   * frames did arrive, the question is asked again with the new number.
   */
  async checkHealth(): Promise<void> {
    const link = this.#link;
    const generation = this.#generation;
    if (this.#checking || !link || !this.#checkable(link, generation)) return;
    this.#checking = true;
    try {
      for (let attempt = 0; attempt < HEALTH_ATTEMPTS; attempt += 1) {
        const seq = this.#lastSeq;
        let healthy: boolean;
        try {
          healthy = await link.check(generation, seq);
        } catch {
          // No answer says nothing about the frames. If the backend is out
          // of reach, the socket's close or the next interest call says so.
          return;
        }
        if (healthy || !this.#checkable(link, generation)) return;
        if (this.#lastSeq === seq) {
          await new Promise((resolve) => setTimeout(resolve, HEALTH_CONFIRM_MS));
          if (!this.#checkable(link, generation)) return;
          if (this.#lastSeq === seq) {
            console.warn('[sessionStream] The stream is behind the backend; starting over');
            this.#restart();
            return;
          }
        }
      }
    } finally {
      this.#checking = false;
    }
  }

  // ── Availability ──────────────────────────────────────────────────────────

  /** The stream stopped being established. Consumers get a grace period. */
  #lost(): void {
    if (this.#availability === 'established') this.#setAvailability('connecting');
  }

  /**
   * The grace period ran out. What the frames brought is not being kept
   * current any more and nothing else brings it, so it goes: a row falls
   * back to its static label. Starting over means that whatever establishes
   * the stream next (a socket opening, a retry that is accepted, a call
   * that was only slow) is answered with snapshots.
   */
  #giveUp(): void {
    for (const [sessionId, entry] of this.#entries) this.#forget(sessionId, entry);
    this.#restart();
    this.#setAvailability('unavailable');
  }

  #setAvailability(next: SessionStreamAvailability): void {
    if (this.#availability === next) return;
    this.#availability = next;
    if (this.#graceTimer !== null) clearTimeout(this.#graceTimer);
    this.#graceTimer = null;
    if (next === 'connecting') {
      this.#graceTimer = setTimeout(() => {
        this.#graceTimer = null;
        this.#giveUp();
      }, ESTABLISH_GRACE_MS);
    }
    for (const listener of [...this.#availabilityListeners]) listener(next);
  }
}

/** This window's owner. */
export const sessionStream = new SessionStreamOwner();

/** Start this window's session stream. Returns the function that stops it. */
export function startSessionStream(): () => void {
  sessionStream.start();
  return () => sessionStream.stop();
}
