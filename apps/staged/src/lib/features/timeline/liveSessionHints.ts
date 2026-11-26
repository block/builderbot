import type { BranchTimeline } from '../../types';
import type { DisplayRootInput } from '../sessions/pathDisplayRoots';
import { sessionStream, type SessionStreamOwner } from '../../stores/sessionStream.svelte';

// The hints and titles of running sessions, for the view that shows them.
//
// They come from the session stream and nowhere else. A view hands the
// stream's owner (`stores/sessionStream.svelte.ts`) the sessions it shows,
// and each of its rows reads its own session's title and subtitle from the
// owner. Nothing is fetched here and there is no poll behind it: while the
// stream is being (re)established a row keeps what it showed, and once the
// stream has been given up on it shows its static label until it is back
// (see `createLiveSessionRows`).

export type PendingHintItemType = 'pending-commit' | 'generating-note' | 'generating-review';

export type PendingHintItem = {
  type: PendingHintItemType;
  sessionId?: string;
};

export function fallbackHintForPendingType(type: string): string {
  if (type === 'pending-commit') return 'Generating commit';
  if (type === 'generating-review') return 'Generating review';
  if (type === 'queued-commit') return 'Queued commit';
  if (type === 'queued-review') return 'Queued review';
  if (type === 'queued-note') return 'Queued note';
  return 'Generating note';
}

export function collectRunningSessionIds(
  timeline: BranchTimeline,
  pendingItems: { sessionId?: string }[]
): string[] {
  const ids = new Set<string>();

  for (const commit of timeline.commits) {
    if (commit.sessionStatus === 'running' && commit.sessionId) {
      ids.add(commit.sessionId);
    }
  }
  for (const note of timeline.notes) {
    if (note.sessionStatus === 'running' && note.sessionId) {
      ids.add(note.sessionId);
    }
  }
  for (const review of timeline.reviews) {
    if (review.sessionStatus === 'running' && review.sessionId) {
      ids.add(review.sessionId);
    }
  }
  for (const item of pendingItems) {
    if (item.sessionId) {
      ids.add(item.sessionId);
    }
  }

  return Array.from(ids);
}

export interface LiveSessionRowsOptions {
  /**
   * Whether the view's rows can be seen. A view that stays mounted while its
   * workspace layer is covered or parked says so here, and its sessions are
   * let go of meanwhile.
   */
  visible?: () => boolean;
  /** The roots the view's tool paths are shortened against. */
  displayRoots?: () => DisplayRootInput;
  /** The stream's owner. This window's, unless a test stands in for it. */
  owner?: SessionStreamOwner;
}

/**
 * A view's running sessions on the session stream.
 *
 * `sync` says which sessions the view shows, and `hint` and `title` are what
 * a row shows for one of them. Both are reactive reads of that session's own
 * values in the stream's owner, so a row is evaluated again when its own
 * session changes and for no other session's frames. `sync` reads `visible`
 * and `displayRoots`, so call it from an effect; call `hint` and `title`
 * only for sessions given to `sync`, since a read of a session the owner
 * does not hold waits on every session's changes.
 *
 * What a row shows when the stream has nothing for its session:
 *
 * - Nothing has arrived yet (the session was just taken up, its subtitle is
 *   not derived yet, or the view was off screen for longer than the owner
 *   keeps a session nobody shows): the row keeps what it last showed.
 * - The view is off screen: the same, and the owner is not read at all.
 * - The stream is unavailable (it could not be established within its grace
 *   period, the hub has stopped, the interest keeps being refused): the
 *   owner drops what it held and the last-shown values are dropped here, so
 *   the row shows its static label ("Generating note" and the stored title)
 *   and not a subtitle that has stopped moving. The owner keeps trying, and
 *   the snapshot that comes with the stream brings them back.
 * - The stream says the session is no longer running: nothing, as a row
 *   that is not running shows no hint.
 */
export function createLiveSessionRows(options: LiveSessionRowsOptions = {}) {
  const owner = options.owner ?? sessionStream;
  const consumer = owner.consume();
  // What each row last showed. Plain maps: they are read only where the
  // owner has nothing to say, and the owner's values are what rows wait on.
  const shownHints = new Map<string, string | undefined>();
  const shownTitles = new Map<string, string | undefined>();
  let destroyed = false;

  const stopWatchingStream = owner.onAvailability((availability) => {
    if (availability !== 'unavailable') return;
    shownHints.clear();
    shownTitles.clear();
  });

  const visible = () => options.visible?.() ?? true;

  /**
   * `read` for a session the stream has brought, else what was last shown.
   * `derived` says whether the stream has brought what `read` reads.
   */
  function shown(
    sessionId: string,
    kept: Map<string, string | undefined>,
    derived: () => boolean,
    read: () => string | undefined
  ): string | undefined {
    if (destroyed || !visible()) return kept.get(sessionId);
    const status = owner.status(sessionId);
    if (status === undefined) return kept.get(sessionId);
    let value: string | undefined;
    if (status === 'running') {
      if (!derived()) return kept.get(sessionId);
      value = read();
    }
    kept.set(sessionId, value);
    return value;
  }

  return {
    /**
     * Show `runningSessionIds` with a subtitle and a title, and
     * `titleOnlySessionIds` with their title alone (the stream's summary and
     * title levels). An id in both lists is shown in full.
     */
    sync(runningSessionIds: readonly string[], titleOnlySessionIds: readonly string[] = []) {
      if (destroyed) return;
      const ids = new Set([...runningSessionIds, ...titleOnlySessionIds]);
      for (const kept of [shownHints, shownTitles]) {
        for (const sessionId of [...kept.keys()]) {
          if (!ids.has(sessionId)) kept.delete(sessionId);
        }
      }
      if (!visible()) {
        // Off screen: let go of the sessions. The owner keeps them for a
        // while, so a view shown again soon finds them current.
        consumer.sync({ summary: [], title: [] });
        return;
      }
      consumer.sync({
        summary: runningSessionIds,
        title: titleOnlySessionIds,
        displayRoots: options.displayRoots?.(),
      });
    },
    /** The subtitle of a session given to `sync` as running. */
    hint(sessionId: string): string | undefined {
      const roots = options.displayRoots?.();
      return shown(
        sessionId,
        shownHints,
        () => owner.hintDerived(sessionId, roots),
        () => owner.hint(sessionId, roots)
      );
    },
    /** The ACP title of a session given to `sync`, cleaned up for display. */
    title(sessionId: string): string | undefined {
      return shown(
        sessionId,
        shownTitles,
        () => true,
        () => owner.title(sessionId)
      );
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
      stopWatchingStream();
      consumer.release();
      shownHints.clear();
      shownTitles.clear();
    },
  };
}

export type LiveSessionRows = ReturnType<typeof createLiveSessionRows>;
