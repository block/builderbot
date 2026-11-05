/**
 * The title index project search matches note, project-note and review
 * titles against: `SearchContext.index` for both search surfaces.
 *
 * Titles only, read from SQLite in one backend call, so it loads on demand —
 * when a search field takes focus or a query starts — rather than at startup.
 * Matching stays synchronous: the index is a plain map once loaded, and a
 * keystroke never waits on it; results found through a title appear once it
 * lands.
 *
 * Freshness comes from the store change feed. A note, review, branch or
 * project write marks the index stale, and so does anything that may have
 * lost such an event: a listener going live after the first read started
 * (Tauri registers asynchronously), a web-socket (re)connect, and the
 * page-resume `cache-stale` recovery. While a search is active a stale index
 * refetches, coalesced so a burst of writes (a session filling in a note, a
 * branch move) costs one read; otherwise it waits for the next search. The
 * stale index keeps matching until the fresh one lands, so results never
 * blank out mid-search.
 *
 * The search fields call ensureLoaded on focus and on every query change.
 * That loads at once only when nothing else will: the index has never
 * loaded, the last load failed (so a failure mid-search retries on the next
 * keystroke), or a write marked it stale while no search ran (so the
 * keystroke that starts one picks it up). Staleness from a write during a
 * search is the refetch timer's, so typing through a burst of writes still
 * costs one read per REFETCH_DELAY_MS, not one per keystroke.
 */

import { getProjectSearchIndex } from '../../commands';
import { listenToEvent, type UnlistenFn } from '../../transport';
import type { ProjectSearchItem } from '../../types';
import type { ProjectSearchIndex, ProjectSearchIndexItem } from './projectSearch';
import { projectsListViewState } from './projectsListViewState.svelte';

/** How long a stale index waits for more writes before refetching. */
export const REFETCH_DELAY_MS = 500;

/** The change-feed events after which a title may have changed or moved. */
const STALE_EVENTS = ['notes-changed', 'review-changed', 'branch-changed', 'project-changed'];

/** The backend's flat item list grouped by project, titles lower-cased once. */
export function groupSearchItems(items: readonly ProjectSearchItem[]): ProjectSearchIndex {
  const byProject = new Map<string, ProjectSearchIndexItem[]>();
  for (const item of items) {
    let list = byProject.get(item.projectId);
    if (!list) byProject.set(item.projectId, (list = []));
    list.push({
      kind: item.kind,
      id: item.id,
      title: item.title,
      text: item.title.toLowerCase(),
      updatedAt: item.updatedAt,
    });
  }
  return byProject;
}

export class ProjectSearchIndexStore {
  /** Null until the first load lands; stale but usable after that. */
  private _index = $state<ProjectSearchIndex | null>(null);
  /** Whether the backend may hold titles this index doesn't. */
  private stale = true;
  /** Whether the last load failed; cleared by the next one that lands. */
  private failed = false;
  private inFlight: Promise<void> | null = null;
  /** A load asked for while one was in flight: run it when that one lands. */
  private queued = false;
  private refetchTimer: ReturnType<typeof setTimeout> | null = null;
  private unlisteners: UnlistenFn[] = [];

  /** `searching` says whether a query is active, which is when staleness
   *  is worth a refetch rather than a note for the next search. */
  constructor(private readonly searching: () => boolean) {}

  get index(): ProjectSearchIndex | null {
    return this._index;
  }

  /**
   * Load the index now if it has never loaded, the last load failed, or it
   * went stale with no refetch scheduled — a write while no search ran.
   * Called when a search field takes focus (prefetch, so the first keystroke
   * already has titles) and on every query change. Repeat calls while fresh,
   * in flight or waiting on the refetch timer are no-ops.
   */
  ensureLoaded(): void {
    this.listen();
    if (!this.stale) return;
    // A write during a search scheduled a coalesced refetch: leave it to
    // that, unless there's no index to match with or the last read failed.
    if (this.refetchTimer && this._index !== null && !this.failed) return;
    this.load();
  }

  /** Tear down the feed listeners (tests, symmetry with ensureLoaded). */
  stop(): void {
    for (const unlisten of this.unlisteners) unlisten();
    this.unlisteners = [];
    if (this.refetchTimer) clearTimeout(this.refetchTimer);
    this.refetchTimer = null;
  }

  private listen(): void {
    if (this.unlisteners.length > 0) return;
    const markStale = () => this.markStale();
    for (const event of STALE_EVENTS) {
      // onEstablished: events sent before the listener went live (or while
      // the web socket was down) are gone, and the read may predate them.
      this.unlisteners.push(listenToEvent(event, markStale, { onEstablished: markStale }));
    }
    // A page freeze or socket gap loses change-feed events for good; the
    // recovery for that is this event, which every feed-fed store follows.
    window.addEventListener('cache-stale', markStale);
    this.unlisteners.push(() => window.removeEventListener('cache-stale', markStale));
  }

  private markStale(): void {
    this.stale = true;
    if (this.refetchTimer || !this.searching()) return;
    this.refetchTimer = setTimeout(() => {
      this.refetchTimer = null;
      // The search may have ended while the timer ran; then the next
      // ensureLoaded picks the staleness up.
      if (this.stale && this.searching()) this.load();
    }, REFETCH_DELAY_MS);
  }

  private load(): void {
    if (this.inFlight) {
      this.queued = true;
      return;
    }
    // Cleared at the start, not the end: a write that lands during the read
    // marks it stale again, which is right, since the read may predate it.
    this.stale = false;
    this.inFlight = getProjectSearchIndex()
      .then((items) => {
        this._index = groupSearchItems(items);
        this.failed = false;
      })
      .catch((e) => {
        // Retry on the next ensureLoaded — a keystroke, focus or new search —
        // rather than in a loop.
        this.stale = true;
        this.failed = true;
        console.error('[projectSearchIndex] Failed to load the search index:', e);
      })
      .finally(() => {
        this.inFlight = null;
        if (this.queued) {
          this.queued = false;
          this.load();
        }
      });
  }
}

export const projectSearchIndex = new ProjectSearchIndexStore(
  () => projectsListViewState.searchQuery.trim().length > 0
);
