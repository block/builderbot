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
 * project write marks the index stale. While a search is active a stale
 * index refetches, coalesced so a burst of writes (a session filling in a
 * note, a branch move) costs one read; otherwise it waits for the next
 * search. The stale index keeps matching until the fresh one lands, so
 * results never blank out mid-search.
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
   * Load the index if it has never loaded or a write has gone stale since.
   * Called when a search field takes focus (prefetch, so the first keystroke
   * already has titles) and when a query starts. Repeat calls while fresh or
   * in flight are no-ops.
   */
  ensureLoaded(): void {
    this.listen();
    if (this.stale) this.load();
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
    for (const event of STALE_EVENTS) {
      this.unlisteners.push(listenToEvent(event, () => this.markStale()));
    }
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
      })
      .catch((e) => {
        // Retry on the next search rather than in a loop.
        this.stale = true;
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
