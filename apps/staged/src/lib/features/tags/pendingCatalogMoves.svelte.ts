/**
 * The catalog reorders that show before their writes land: tag moves and
 * group reorders, from any surface.
 *
 * tagActions queues a catalog write, awaits the backend and only then
 * patches tagsStore. A reorder has to show at once, so each one made through
 * here is also kept as a pending entry, and `catalog` is the store's
 * snapshot with the pending entries applied on top, in order. A surface that
 * resolves its next reorder against `catalog` steps from where the last one
 * left things: Move down pressed three times quickly moves three places. An
 * entry leaves once its write lands, since the store then holds it.
 *
 * A failed write clears every entry and starts a new generation: the later
 * ones were resolved against an order that never happened, so the ones still
 * queued are skipped, not sent. That holds across surfaces. There is one
 * catalog write chain, so there is one overlay, at module scope like the
 * chain itself.
 *
 * An entry is a snapshot patch. tagActions applies the same patch to the
 * store when the write lands, a moment before the entry goes, so a patch
 * must leave a snapshot it was already applied to unchanged.
 */

import type { TagState } from '../../types';
import { tagsStore } from '../../stores/tags.svelte';
import { tagActions, type CatalogWriteHooks } from './tagActions.svelte';
import { withGroupOrder, withTagMoved, type TagMove } from './tagCatalogPatches';
import { indexTagCatalog, type TagCatalog } from './tagState';

/** A reorder in flight. The token finds it again once its write settles. */
interface PendingEntry {
  token: number;
  patch: (state: TagState) => TagState;
}

class PendingCatalogMoves {
  /**
   * Entries whose writes are in flight, oldest first. Raw, and replaced
   * wholesale: a deep `$state` proxy would hand back proxies, not the objects
   * pushed in, and the list is only ever read as a whole.
   */
  private pending = $state.raw<readonly PendingEntry[]>([]);
  /** Bumped when a write fails; entries queued under an older one are skipped. */
  private generation = 0;
  private nextToken = 0;

  /** The store's snapshot with the pending entries applied on top. */
  private readonly view = $derived(
    this.pending.reduce((state, { patch }) => patch(state), tagsStore.snapshot)
  );

  /** The catalog as it will stand once every pending write has landed. */
  readonly catalog: TagCatalog = $derived(indexTagCatalog(this.view));

  /**
   * Move a tag within its scope or into another, showing it at once.
   * Resolves to whether it moved (false for a failure or a skip).
   */
  moveTag(move: TagMove): Promise<boolean> {
    return this.show(
      (state) => withTagMoved(state, move),
      (hooks) => tagActions.moveTag(move, hooks)
    );
  }

  /**
   * Reorder the groups, showing it at once; `groupIds` must list every group
   * once. Resolves to whether it landed (false for a failure or a skip).
   */
  reorderGroups(groupIds: readonly string[]): Promise<boolean> {
    const order = [...groupIds];
    return this.show(
      (state) => withGroupOrder(state, order),
      (hooks) => tagActions.reorderTagGroups(order, hooks)
    );
  }

  /** Show `patch` until `write`, queued on the catalog chain, settles. */
  private show(
    patch: (state: TagState) => TagState,
    write: (hooks: CatalogWriteHooks) => Promise<boolean>
  ): Promise<boolean> {
    const generation = this.generation;
    const token = this.nextToken++;
    this.pending = [...this.pending, { token, patch }];
    // The bookkeeping runs inside tagActions' chain, so a failure is seen
    // here before the next queued write asks whether it's still wanted.
    return write({
      stillWanted: () => this.generation === generation,
      onSettled: (ok) => {
        // An earlier write of this generation failed: the overlay already
        // went, and this one was skipped.
        if (this.generation !== generation) return;
        if (ok) {
          // Once it lands the store's own patch holds the reorder, so the
          // entry can go.
          this.pending = this.pending.filter((entry) => entry.token !== token);
        } else {
          // The failure's refetch brings server truth. The later entries
          // were resolved against an order that never happened, so they go
          // too, and the ones still queued see the new generation and
          // aren't sent.
          this.generation++;
          this.pending = [];
        }
      },
    });
  }
}

export const pendingCatalogMoves = new PendingCatalogMoves();
