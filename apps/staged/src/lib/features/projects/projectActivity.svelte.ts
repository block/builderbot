/**
 * When each project last saw activity, for sorting by "Last updated":
 * `ProjectSortContext.activity` for the one sorted list (projectOrder).
 *
 * Read from SQLite in one backend call and derived there on every read, so
 * it loads on demand, only while the window's sort has a last-updated key,
 * rather than at startup. Sorting stays synchronous: until the first read
 * lands a project's last update is its creation time, and the list settles
 * into place when it does.
 *
 * Freshness comes from the store change feed, as for the search index
 * (projectSearchIndex). A note, review, branch or project write marks the
 * map stale (commit writes publish `branch-changed`), and so does anything
 * that may have lost such an event: a listener going live after the first
 * read started, a web-socket (re)connect, and the page-resume `cache-stale`
 * recovery. While the sort uses it a stale map refetches, coalesced so a
 * burst of writes (a session filling in a note, a branch move) costs one
 * read; otherwise it waits for ensureLoaded, which the sorted list calls
 * when a last-updated key comes back. A session finishing writes its note,
 * review or commit, which publishes one of those events, so there is no
 * session listener.
 *
 * A failed read keeps the last map and is retried, backing off, for as long
 * as the sort uses it. Nothing else would ask again: unlike a search, a
 * sort has no keystrokes. The retry is the next read whatever else asks for
 * one meanwhile, so a write during the failing read does not skip the wait.
 * A sign that the backend is reachable again (a listener going live, a
 * socket reconnect, `cache-stale`) ends the wait and starts the backoff
 * over.
 *
 * Two things keep rows still. A read that changes no timestamp keeps the
 * same map, so nothing re-sorts. And nothing moves under the pointer: a
 * fresh map is held while a pointer is down or a card is being dragged, and
 * applied once it is released. WebKit drops the click on a row that moves between
 * press and release (searchRowOrder holds its re-key for the same reason),
 * and a card reordered mid-drag loses its drag. Every press and drag this
 * sees start is also seen to end, including a drag refused at its start and
 * a press whose release a context menu takes, so the hold never outlasts
 * them.
 */

import { getProjectActivity } from '../../commands';
import { listenToEvent, type UnlistenFn } from '../../transport';
import type { ProjectActivity } from '../../types';
import { sortUsesActivity } from './projectSort';
import { projectSortPrefs } from './projectSortPrefs.svelte';

/** How long a stale map waits for more writes before refetching. */
export const REFETCH_DELAY_MS = 500;

/** How long a failed read waits before the first retry; doubles per failure. */
export const RETRY_DELAY_MS = 2_000;
/** The longest wait between retries. */
export const MAX_RETRY_DELAY_MS = 60_000;

/** The change-feed events after which a project's last activity may have moved. */
const STALE_EVENTS = ['notes-changed', 'review-changed', 'branch-changed', 'project-changed'];

/**
 * What ends a press: the click it makes, or the release or cancel when there
 * is none. And a context menu, which can take the release: a native menu
 * dismissed with Escape sends no pointerup at all. Nothing is clicked by a
 * press that opened a menu, so there is no click to keep a row still for.
 */
const PRESS_END_EVENTS = ['pointerup', 'pointercancel', 'click', 'contextmenu'] as const;
/**
 * The press ends that also mean no drag is in flight. A native drag takes
 * the pointer when it starts (`pointercancel`) and ends with neither of
 * these, so one arriving after a `dragstart` means that drag never began.
 */
const NO_DRAG_EVENTS: ReadonlySet<string> = new Set(['pointerup', 'click']);
/** What ends a card's drag: the drop, or the drag's own end when it lands nowhere. */
const DRAG_END_EVENTS = ['dragend', 'drop'] as const;

export type ProjectActivityMap = ReadonlyMap<string, number>;

/** The backend's rows as a map from project id to its last activity. */
export function indexProjectActivity(rows: readonly ProjectActivity[]): Map<string, number> {
  return new Map(rows.map((row) => [row.projectId, row.lastActivityAt]));
}

function sameActivity(a: ProjectActivityMap, b: ProjectActivityMap): boolean {
  if (a.size !== b.size) return false;
  for (const [projectId, at] of a) if (b.get(projectId) !== at) return false;
  return true;
}

export class ProjectActivityStore {
  /** Null until the first load lands; stale but usable after that. */
  private _activity = $state.raw<ProjectActivityMap | null>(null);
  /** A map that landed mid-press or mid-drag, waiting for the release. */
  private heldBack: ProjectActivityMap | null = null;
  /** Whether the backend may hold activity this map doesn't. */
  private stale = true;
  /** The wait before the next retry, while the last load failed; else null. */
  private retryDelay: number | null = null;
  private inFlight: Promise<void> | null = null;
  /** A load asked for while one was in flight: run it when that one lands. */
  private queued = false;
  private refetchTimer: ReturnType<typeof setTimeout> | null = null;
  private releaseTimer: ReturnType<typeof setTimeout> | null = null;
  private unlisteners: UnlistenFn[] = [];

  /** A pointer is down. */
  private pressed = false;
  /** A card drag (HTML5) is in flight. */
  private dragging = false;

  /** `wanted` says whether the sort uses the map, which is when staleness
   *  is worth a refetch rather than a note for the next ensureLoaded. */
  constructor(private readonly wanted: () => boolean) {}

  get activity(): ProjectActivityMap | null {
    return this._activity;
  }

  /**
   * Load the map now if it has never loaded, or it went stale with no
   * refetch scheduled: a write while the sort didn't use it. Called when the
   * sort gains a last-updated key (projectOrder). Repeat calls while fresh,
   * in flight or waiting on the refetch timer are no-ops.
   */
  ensureLoaded(): void {
    this.listen();
    if (!this.stale) return;
    // A write scheduled a coalesced refetch, or a failure its retry: leave
    // it to that, unless there's no map to sort by yet.
    if (this.refetchTimer && this._activity !== null) return;
    this.load();
  }

  /** Tear down the listeners and timers (tests, symmetry with ensureLoaded). */
  stop(): void {
    for (const unlisten of this.unlisteners) unlisten();
    this.unlisteners = [];
    this.clearRefetchTimer();
    if (this.releaseTimer) clearTimeout(this.releaseTimer);
    this.releaseTimer = null;
    this.pressed = false;
    this.dragging = false;
  }

  private listen(): void {
    if (this.unlisteners.length > 0) return;
    const markStale = () => this.markStale();
    const recovered = () => this.recovered();
    for (const event of STALE_EVENTS) {
      // onEstablished: events sent before the listener went live (or while
      // the web socket was down) are gone, and the read may predate them.
      this.unlisteners.push(listenToEvent(event, markStale, { onEstablished: recovered }));
    }
    // A page freeze or socket gap loses change-feed events for good; the
    // recovery for that is this event, which every feed-fed store follows.
    this.on('cache-stale', recovered);

    // Window-wide and capturing, so a press or drag is seen wherever it
    // starts and ends, whatever its handlers stop.
    this.on(
      'pointerdown',
      () => {
        this.pressed = true;
        // A new press means any drag is over, even one whose end was never
        // seen: a card that left the page mid-drag gets no dragend.
        this.dragging = false;
      },
      true
    );
    const endPress = () => {
      this.pressed = false;
      this.release();
    };
    // A drag refused at its start has no dragend or drop to end it. A column
    // handle refuses the native drag (pointerDrag), but only after the
    // capturing listener below has seen its dragstart. The press it came
    // from then ends as any other does, and that ends the drag here too.
    const endPressAndDrag = () => {
      this.dragging = false;
      endPress();
    };
    for (const type of PRESS_END_EVENTS) {
      this.on(type, NO_DRAG_EVENTS.has(type) ? endPressAndDrag : endPress, true);
    }
    // A press whose end the page never sees (⌘-Tab away mid-press) ends when
    // the window loses focus. Not capturing: blur doesn't bubble, so a plain
    // listener hears only the window's own, not every element's.
    this.on('blur', endPress);
    this.on(
      'dragstart',
      () => {
        this.dragging = true;
        // The drag takes the press over: its pointerup never comes.
        this.pressed = false;
      },
      true
    );
    const endDrag = () => {
      this.dragging = false;
      this.release();
    };
    for (const type of DRAG_END_EVENTS) this.on(type, endDrag, true);
  }

  private on(type: string, handler: () => void, capture = false): void {
    window.addEventListener(type, handler, capture);
    this.unlisteners.push(() => window.removeEventListener(type, handler, capture));
  }

  /**
   * Apply the map that landed mid-press or mid-drag, once neither holds. In
   * a task of its own: the press's click and the drag's drop are dispatched
   * after the event that ends them, and a row moved before then loses them.
   */
  private release(): void {
    if (this.heldBack === null || this.releaseTimer) return;
    this.releaseTimer = setTimeout(() => {
      this.releaseTimer = null;
      // Another press may have started since.
      if (this.pressed || this.dragging || this.heldBack === null) return;
      this._activity = this.heldBack;
      this.heldBack = null;
    }, 0);
  }

  private landed(rows: readonly ProjectActivity[]): void {
    const next = indexProjectActivity(rows);
    // Whatever was waiting is superseded, shown or not.
    this.heldBack = null;
    if (this._activity !== null && sameActivity(this._activity, next)) return;
    if (this.pressed || this.dragging) this.heldBack = next;
    else this._activity = next;
  }

  private markStale(): void {
    this.stale = true;
    this.scheduleRefetch(REFETCH_DELAY_MS);
  }

  /**
   * Events may have been lost, and the backend can be reached again: a
   * listener went live, the socket reconnected or the page resumed. Stale as
   * after any write. A retry that was backing off also stops waiting, since
   * the gap that just ended is the likeliest cause of its failures: without
   * this a list could stay out of date for up to MAX_RETRY_DELAY_MS after
   * the connection came back.
   */
  private recovered(): void {
    if (this.retryDelay !== null) {
      this.retryDelay = null;
      this.clearRefetchTimer();
    }
    this.markStale();
  }

  /** Refetch after `delay`, unless a refetch is already waiting or nobody sorts by the map. */
  private scheduleRefetch(delay: number): void {
    if (this.refetchTimer || !this.wanted()) return;
    this.refetchTimer = setTimeout(() => {
      this.refetchTimer = null;
      // The sort may have dropped its key while the timer ran; then the
      // next ensureLoaded picks the staleness up.
      if (this.stale && this.wanted()) this.load();
    }, delay);
  }

  private clearRefetchTimer(): void {
    if (this.refetchTimer) clearTimeout(this.refetchTimer);
    this.refetchTimer = null;
  }

  private load(): void {
    if (this.inFlight) {
      this.queued = true;
      return;
    }
    // This read covers whatever a waiting refetch or retry was for.
    this.clearRefetchTimer();
    // Cleared at the start, not the end: a write that lands during the read
    // marks it stale again, which is right, since the read may predate it.
    this.stale = false;
    let failed = false;
    this.inFlight = getProjectActivity()
      .then((rows) => {
        this.retryDelay = null;
        this.landed(rows);
      })
      .catch((e) => {
        failed = true;
        this.stale = true;
        console.error('[projectActivity] Failed to load project activity:', e);
        const delay = this.retryDelay ?? RETRY_DELAY_MS;
        this.retryDelay = Math.min(delay * 2, MAX_RETRY_DELAY_MS);
        // In place of a refetch a write scheduled during the read, which
        // would otherwise be the retry, without the wait.
        this.clearRefetchTimer();
        this.scheduleRefetch(delay);
      })
      .finally(() => {
        this.inFlight = null;
        const queued = this.queued;
        this.queued = false;
        // After a failure the retry is the next read, and it covers whatever
        // was queued: running that now would skip the wait.
        if (queued && !failed) this.load();
      });
  }
}

export const projectActivity = new ProjectActivityStore(() =>
  sortUsesActivity(projectSortPrefs.sort)
);
