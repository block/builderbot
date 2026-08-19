/**
 * Page Lifecycle integration for cache staleness detection.
 *
 * Listens for `resume`, `visibilitychange`, `pagehide`, and `pageshow` events to
 * detect when the browser tab (or iOS PWA) has been frozen and restored. When a
 * significant time gap is detected, all IndexedDB cache entries are marked stale
 * and a `cache-stale` CustomEvent is dispatched so components revalidate.
 *
 * Revalidation is *gated on how long the page was hidden*: a brief tab switch
 * (a few seconds) keeps the cached data as-is, so resuming does not trigger a
 * network revalidation storm across every cached entry. Only a hide longer than
 * the threshold forces a full revalidation.
 *
 * Going hidden also persists a synchronous snapshot of the in-memory timeline
 * cache (see commands.ts) — iOS tears the tab down while hidden, so this is the
 * last chance to capture state for the next cold boot's first-frame paint.
 *
 * The revalidation itself (`revalidateAll`) is exported, because page resume is
 * no longer its only trigger: the web-mode event socket calls it on reconnect
 * (transport.ts), where every event emitted during the socket gap is lost for
 * good. It is the shared "assume everything since the last known-good point was
 * missed" recovery — the gating differs per caller, the recovery doesn't.
 */

import { isTauri } from '../transport';
import { markAllStale } from '../cache';
import { persistTimelineSnapshot } from '../commands';

const STALE_THRESHOLD_MS = 30_000;

/**
 * Minimum hidden duration before a resume forces a full revalidation. Chosen so
 * a quick app/tab switch (the common case on iOS) keeps the just-cached data,
 * while a longer absence — where branches, commits, or PR state have likely
 * moved on — revalidates everything.
 */
const RESUME_REVALIDATE_THRESHOLD_MS = STALE_THRESHOLD_MS;

let lastActivityTimestamp = Date.now();
/** When the page last went hidden (0 = not hidden / unknown). */
let hiddenAt = 0;

/** True when we have no recorded hide, or the hide outlasted the threshold. */
function shouldRevalidateAfterResume(): boolean {
  return hiddenAt === 0 || Date.now() - hiddenAt > RESUME_REVALIDATE_THRESHOLD_MS;
}

/**
 * Recover from an unknown-length gap in event delivery: every cached entry is
 * marked stale so the next read revalidates over the network, and mounted views
 * refetch immediately.
 *
 * Callers: page resume (gated on the hidden duration, below) and web-mode
 * WebSocket reconnect (transport.ts), where the store change feed, PR-poll and
 * session events emitted while the socket was down are unrecoverable — the
 * server keeps no per-client queue.
 *
 * `markAllStale()` must precede the `cache-stale` dispatch: an unmarked SWR hit
 * would serve a cached list with no revalidating leg, so the handlers would
 * refetch nothing.
 */
export async function revalidateAll() {
  await markAllStale();
  window.dispatchEvent(new CustomEvent('cache-stale'));
  // Project notes are the one surface with no `cache-stale` consumer: they
  // refetch only on this event, so a note change missed during the gap would
  // stay invisible until the next notes-changed.
  window.dispatchEvent(new CustomEvent('project-notes-invalidated'));
}

/** Record the page going hidden and snapshot caches for the next cold boot. */
function handleHidden() {
  hiddenAt = Date.now();
  lastActivityTimestamp = hiddenAt;
  // Capture synchronously now — on iOS the tab may be torn down before any
  // async work (IndexedDB) could complete.
  persistTimelineSnapshot();
}

async function handleResume() {
  // The Page Lifecycle `resume` event follows a real freeze, but still gate on
  // the hidden duration so a brief freeze doesn't force a full revalidation.
  const revalidate = shouldRevalidateAfterResume();
  lastActivityTimestamp = Date.now();
  hiddenAt = 0;
  if (revalidate) await revalidateAll();
}

async function handleVisibilityChange() {
  if (document.visibilityState === 'visible') {
    const now = Date.now();
    if (now - lastActivityTimestamp > STALE_THRESHOLD_MS) {
      await revalidateAll();
    }
    lastActivityTimestamp = now;
    hiddenAt = 0;
  } else {
    handleHidden();
  }
}

function handlePageHide() {
  handleHidden();
}

async function handlePageShow(event: PageTransitionEvent) {
  // bfcache restore: JS context survived but data may be stale. Only revalidate
  // when the page was hidden long enough for the data to plausibly be stale —
  // otherwise a 2-second tab switch would force a network re-fetch of everything.
  if (!event.persisted) return;
  const revalidate = shouldRevalidateAfterResume();
  lastActivityTimestamp = Date.now();
  hiddenAt = 0;
  if (revalidate) await revalidateAll();
}

/**
 * Start listening for page lifecycle events. Returns an unlisten function.
 * No-ops in Tauri mode (no page eviction).
 */
export function listenForPageLifecycle(): () => void {
  if (isTauri) return () => {};

  document.addEventListener('resume', handleResume);
  document.addEventListener('visibilitychange', handleVisibilityChange);
  window.addEventListener('pagehide', handlePageHide);
  window.addEventListener('pageshow', handlePageShow);

  return () => {
    document.removeEventListener('resume', handleResume);
    document.removeEventListener('visibilitychange', handleVisibilityChange);
    window.removeEventListener('pagehide', handlePageHide);
    window.removeEventListener('pageshow', handlePageShow);
  };
}

// Exported for testing
export function _setLastActivityTimestamp(ts: number) {
  lastActivityTimestamp = ts;
}
export function _getLastActivityTimestamp() {
  return lastActivityTimestamp;
}
export function _setHiddenAt(ts: number) {
  hiddenAt = ts;
}
export function _getHiddenAt() {
  return hiddenAt;
}
export { STALE_THRESHOLD_MS as _STALE_THRESHOLD_MS };
