/**
 * Project tags: the tag catalog (groups and tags) and which projects carry
 * which tags.
 *
 * Module-scoped runes store holding the backend's whole `get_tag_state`
 * snapshot — small enough to refetch on every `tags-changed` — plus an
 * overlay of pending assignment writes, so a pick repaints every surface at
 * once. Tags live apart from the project list on purpose: a tag write
 * publishes `tags-changed`, never `project-changed`, whose hard reload
 * restarts every project's branch and repo hydration.
 *
 * Freshness is a seq-guarded snapshot. Each `get_tag_state` request takes
 * the next `fetchSeq`, and its response applies only when it is newer than
 * the last one applied and than `discardThroughSeq` — which `patchLocal`
 * moves past every request in flight, since those can't contain the write it
 * just applied. An intent (see tagActions) stays until a snapshot requested
 * after its write committed lands, so neither a stale refetch nor the gap
 * before the echo flashes the old value.
 *
 * The pure rules live in features/tags/tagState.ts. startListeners() and the
 * first load() are wired once from App.svelte.
 */

import { listenToEvent, type UnlistenFn } from '../transport';
import * as commands from '../commands';
import type { Tag, TagGroup, TagState, TagsChangedEvent } from '../types';
import {
  EMPTY_TAG_STATE,
  chipTags,
  effectiveProjectTagIds,
  groupTag,
  indexAssignments,
  indexTagCatalog,
  intentSlot,
  pruneConfirmedIntents,
  usageCounts,
  withConfirmedIntent,
  withIntent,
  withoutIntent,
  type IntentOverlay,
  type TagCatalog,
  type TagIntent,
  type TagSlot,
} from '../features/tags/tagState';
import { projectsDataStore } from './projectsData.svelte';

const EMPTY_TAG_IDS: ReadonlySet<string> = new Set();

class TagsStore {
  private _snapshot = $state.raw<TagState>(EMPTY_TAG_STATE);
  private _loaded = $state(false);
  private _intents = $state.raw<IntentOverlay>(new Map());

  /** Seq of the newest `get_tag_state` request issued. */
  private fetchSeq = 0;
  /** Seq of the newest network snapshot applied. */
  private appliedSeq = 0;
  /** Responses to requests up to this seq predate a local patch: ignored. */
  private discardThroughSeq = 0;
  private nextToken = 0;

  private initialLoad: Promise<void> | null = null;
  private refreshInFlight: Promise<void> | null = null;
  private refreshQueued: Promise<void> | null = null;

  private listening = false;
  private unlisteners: UnlistenFn[] = [];

  // ── Reactive reads ──

  readonly catalog: TagCatalog = $derived(indexTagCatalog(this._snapshot));

  private readonly serverProjectTagIds = $derived(
    indexAssignments(this._snapshot, this.catalog.tagsById)
  );

  /**
   * Each project's tags: the server's assignments, minus any to tags not in
   * the catalog, with pending writes applied. Projects with no tags are absent.
   */
  readonly projectTagIds: ReadonlyMap<string, ReadonlySet<string>> = $derived(
    effectiveProjectTagIds(this.serverProjectTagIds, this._intents, this.catalog.tagsById)
  );

  /** How many live projects (no delete in flight) carry each tag. */
  readonly usageCount: ReadonlyMap<string, number> = $derived(
    usageCounts(
      this.projectTagIds,
      projectsDataStore.liveProjects.map((project) => project.id)
    )
  );

  /**
   * Whether the first load settled. Until then a project's icon-group tag is
   * unknown, so its status icon shows a placeholder rather than guessing. A
   * failed first load settles too, leaving every project on its computed
   * status until a refresh lands.
   */
  get loaded(): boolean {
    return this._loaded;
  }

  /**
   * The backend's snapshot as last applied or patched, without pending
   * assignment writes. For editors that layer their own optimistic view over
   * the catalog (Settings › Tags' drag reorder).
   */
  get snapshot(): TagState {
    return this._snapshot;
  }

  /** Every group, in position order. */
  get groups(): TagGroup[] {
    return this.catalog.groups;
  }

  get tagsById(): ReadonlyMap<string, Tag> {
    return this.catalog.tagsById;
  }

  /** Each group's tags in order, keyed by group id; `null` holds the ungrouped tags. */
  get tagsByGroup(): ReadonlyMap<string | null, Tag[]> {
    return this.catalog.tagsByGroup;
  }

  /** The group whose tag stands in for the computed status icon, if any. */
  get iconGroup(): TagGroup | null {
    return this.catalog.iconGroup;
  }

  /** The groups whose boards are pinned to the sidebar, in position order. */
  get sidebarGroups(): TagGroup[] {
    return this.catalog.sidebarGroups;
  }

  tagIdsFor(projectId: string): ReadonlySet<string> {
    return this.projectTagIds.get(projectId) ?? EMPTY_TAG_IDS;
  }

  groupTagFor(projectId: string, groupId: string): Tag | null {
    return groupTag(this.projectTagIds.get(projectId), groupId, this.catalog.tagsById);
  }

  /** The project's tag in the icon group, or null (no icon group, or no tag in it). */
  iconTagFor(projectId: string): Tag | null {
    const iconGroup = this.catalog.iconGroup;
    return iconGroup ? this.groupTagFor(projectId, iconGroup.id) : null;
  }

  /** The project's tags as chips, in group order then ungrouped. */
  chipTagsFor(
    projectId: string,
    options: { excludeGroupIds?: readonly (string | null | undefined)[] } = {}
  ): Tag[] {
    return chipTags(this.projectTagIds.get(projectId), this.catalog, options);
  }

  // ── Loading ──

  /**
   * Load the snapshot. Without `force` this is the startup load, and repeat
   * calls share it; with `force` it is `requestRefresh()`.
   */
  load({ force = false }: { force?: boolean } = {}): Promise<void> {
    if (force) return this.requestRefresh();
    this.initialLoad ??= this.fetchAndApply(false);
    return this.initialLoad;
  }

  /**
   * Refetch from the backend, bypassing the cache. Coalesced like the project
   * list's revalidation: one request in flight and at most one queued behind
   * it, since the one in flight may have read the state before the change
   * that prompted this call. Never rejects.
   */
  requestRefresh(): Promise<void> {
    if (!this.refreshInFlight) {
      this.refreshInFlight = this.fetchAndApply(true).finally(() => {
        this.refreshInFlight = null;
      });
      return this.refreshInFlight;
    }
    this.refreshQueued ??= this.refreshInFlight.then(() => {
      this.refreshQueued = null;
      return this.requestRefresh();
    });
    return this.refreshQueued;
  }

  /**
   * Apply a confirmed catalog write locally (e.g. insert the tag a create
   * returned), then refetch. Every response already in flight was requested
   * before the write, so they are all discarded.
   */
  patchLocal(patch: (state: TagState) => TagState): void {
    this._snapshot = patch(this._snapshot);
    this.discardThroughSeq = this.fetchSeq;
    void this.requestRefresh();
  }

  private async fetchAndApply(force: boolean): Promise<void> {
    const seq = ++this.fetchSeq;
    try {
      const { data, revalidating } = await commands.getTagState({ force });
      // A pending revalidation means `data` came from the (web-only) cache:
      // good for a first paint, but not proof that any write has landed.
      this.applySnapshot(data, seq, revalidating === null);
      if (revalidating) this.applySnapshot(await revalidating, seq, true);
    } catch (e) {
      console.error('[tagsStore] Failed to load tags:', e);
      this._loaded = true;
    }
  }

  private applySnapshot(state: TagState, seq: number, fromNetwork: boolean): void {
    if (seq <= this.discardThroughSeq) return;
    if (fromNetwork) {
      if (seq <= this.appliedSeq) return;
      this.appliedSeq = seq;
      this._intents = pruneConfirmedIntents(this._intents, seq);
    } else if (this.appliedSeq > 0) {
      // A cached value is only a stand-in until the network answers.
      return;
    }
    this._snapshot = state;
    this._loaded = true;
  }

  // ── Pending assignment writes (driven by tagActions) ──

  /**
   * Show `intent` at once, replacing any older intent in its slot. Returns
   * the token the write settles with.
   */
  beginIntent(projectId: string, intent: TagIntent): { slot: TagSlot; token: number } {
    const token = ++this.nextToken;
    this._intents = withIntent(this._intents, projectId, { intent, token, confirmedAtSeq: null });
    return { slot: intentSlot(intent), token };
  }

  /**
   * The write behind `token` committed. If it is still its slot's latest,
   * keep showing it until a snapshot requested from now on lands, and
   * request one.
   */
  confirmIntent(projectId: string, slot: TagSlot, token: number): void {
    const next = withConfirmedIntent(this._intents, projectId, slot, token, this.fetchSeq);
    if (next === this._intents) return;
    this._intents = next;
    void this.requestRefresh();
  }

  /**
   * The write behind `token` failed. If it is still its slot's latest, drop
   * it so the view falls back to server truth; a newer intent survives.
   * Refetches either way, since an older write in the slot may have landed.
   */
  dropIntent(projectId: string, slot: TagSlot, token: number): void {
    this._intents = withoutIntent(this._intents, projectId, slot, token);
    void this.requestRefresh();
  }

  // ── Event listeners ──

  /** Start the app-lifetime listeners. Idempotent; called once from App.svelte. */
  startListeners(): void {
    if (this.listening) return;
    this.listening = true;

    // Store change feed: any tag write, from any window or the backend.
    this.unlisteners.push(
      listenToEvent<TagsChangedEvent>('tags-changed', () => {
        void this.requestRefresh();
      })
    );

    // Page resume and web-socket gaps: events may have been missed.
    const onCacheStale = () => {
      void this.requestRefresh();
    };
    window.addEventListener('cache-stale', onCacheStale);
    this.unlisteners.push(() => window.removeEventListener('cache-stale', onCacheStale));
  }

  /** Tear down the listeners (tests, symmetry with startListeners). */
  stopListeners(): void {
    for (const unlisten of this.unlisteners) unlisten();
    this.unlisteners = [];
    this.listening = false;
  }
}

export const tagsStore = new TagsStore();
