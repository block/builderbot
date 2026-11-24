import * as commands from '../../api/commands';
import type { BranchTimeline, SessionMessage } from '../../types';
import {
  displayRootKey,
  normalizeDisplayRoots,
  resolveDisplayRoots,
  type DisplayRootInput,
} from '../sessions/pathDisplayRoots';
import { sessionStream } from '../../stores/sessionStream.svelte';
import {
  deriveHint,
  derivePipelineHint,
  HINT_ROW_WINDOW,
  normalizeSessionTitle,
} from './sessionHint';

// The hints and titles of running sessions, for the view that shows them.
//
// They come from the session stream (`stores/sessionStream.svelte.ts`) while
// it is there: the view's sessions are handed to the stream's owner, which
// derives each hint from the rows the backend pushes, and nothing is fetched
// here. When the stream is unavailable (the kill switch is off, or it could
// not be established) the poller below does the work as it always has:
// `getSession` plus the transcript's increment per session, every
// `HINT_POLL_INTERVAL_MS`. Both write the same two records, so the view
// cannot tell which one it is on.

const HINT_POLL_INTERVAL_MS = 750;

type HintTracker = {
  lastMessageId: number | null;
  messages: SessionMessage[];
  /**
   * The id of the session's first user message — the prompt that opened it —
   * once the transcript has one. A tracker starts from the whole transcript,
   * so the first user message it sees is the session's first. Every later
   * user message is a reply.
   */
  openingPromptId: number | null;
  rootKey: string;
  displayRoots: string[];
  /**
   * Poll only `getSession` for this id — its ACP title — and never its
   * transcript. A child badge reads the title but nothing reads a child's hint.
   */
  titleOnly: boolean;
};

export type PendingHintItemType = 'pending-commit' | 'generating-note' | 'generating-review';

export type PendingHintItem = {
  type: PendingHintItemType;
  sessionId?: string;
};

function mergeHintMessages(
  previous: SessionMessage[],
  updated: SessionMessage[],
  hadPrevious: boolean
): SessionMessage[] {
  if (updated.length === 0) return previous;
  const merged = hadPrevious ? [...previous.slice(0, -1), ...updated] : updated;
  return merged.slice(-HINT_ROW_WINDOW);
}

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

export function createLiveSessionHints(
  onHintsChange: (hints: Record<string, string>) => void,
  getDisplayRootCandidates?: () => DisplayRootInput,
  onTitlesChange?: (titles: Record<string, string>) => void
) {
  const hintTrackers = new Map<string, HintTracker>();
  let hints: Record<string, string> = {};
  let titles: Record<string, string> = {};
  let hintPollTimer: ReturnType<typeof setInterval> | null = null;
  let hintPollInFlight = false;
  // Paused while the view holding this poller is mounted but off screen (its
  // workspace layer is covered or parked). Trackers and hints are kept so the
  // rows don't blank; only the interval stops.
  let paused = false;
  let destroyed = false;
  // What the view last asked for, whichever source is serving it.
  let fullIds = new Set<string>();
  let titleOnlyIds = new Set<string>();
  // Polling only while the stream cannot be relied on. While it is being
  // (re)established neither source runs and what is shown stays, so a
  // dropped socket does not make every row reload its transcript.
  let polling = sessionStream.availability === 'unavailable';

  function setHint(sessionId: string, hint: string) {
    if (destroyed) return;
    if (hints[sessionId] === hint) return;
    hints = { ...hints, [sessionId]: hint };
    onHintsChange(hints);
  }

  function clearHint(sessionId: string) {
    if (destroyed) return;
    if (!(sessionId in hints)) return;
    const next = { ...hints };
    delete next[sessionId];
    hints = next;
    onHintsChange(hints);
  }

  function setTitle(sessionId: string, title: string) {
    if (destroyed || !onTitlesChange) return;
    if (titles[sessionId] === title) return;
    titles = { ...titles, [sessionId]: title };
    onTitlesChange(titles);
  }

  function clearTitle(sessionId: string) {
    if (destroyed || !onTitlesChange) return;
    if (!(sessionId in titles)) return;
    const next = { ...titles };
    delete next[sessionId];
    titles = next;
    onTitlesChange(titles);
  }

  async function refreshHint(sessionId: string, tracker: HintTracker) {
    if (destroyed) return;
    try {
      const session = await commands.getSession(sessionId);
      // Not the tracker's any more: the view dropped the session, or the
      // stream took over while this was in flight.
      if (destroyed || hintTrackers.get(sessionId) !== tracker) return;
      if (!session || session.status !== 'running') {
        hintTrackers.delete(sessionId);
        clearHint(sessionId);
        clearTitle(sessionId);
        return;
      }

      const acpTitle = normalizeSessionTitle(session.acpTitle);
      if (acpTitle) {
        setTitle(sessionId, acpTitle);
      } else {
        clearTitle(sessionId);
      }

      if (tracker.titleOnly) {
        // The display roots below exist only to format tool-call hints, so a
        // title-only tracker stops here. `clearHint` covers a full-to-title-only
        // flip; otherwise it is a no-op.
        clearHint(sessionId);
        return;
      }

      const rootCandidates: DisplayRootInput = [getDisplayRootCandidates?.(), session.workingDir];
      const nextRootKey = displayRootKey(rootCandidates);
      if (tracker.rootKey !== nextRootKey) {
        tracker.rootKey = nextRootKey;
        tracker.displayRoots = normalizeDisplayRoots(rootCandidates);
        if (nextRootKey) {
          const resolvedRoots = await resolveDisplayRoots(rootCandidates);
          if (
            destroyed ||
            hintTrackers.get(sessionId) !== tracker ||
            tracker.rootKey !== nextRootKey
          ) {
            return;
          }
          tracker.displayRoots = resolvedRoots;
        }
      }

      const updatedMessages =
        tracker.lastMessageId === null
          ? (await commands.getSessionMessages(sessionId)).data
          : await commands.getSessionMessagesSince(sessionId, tracker.lastMessageId);

      if (destroyed || hintTrackers.get(sessionId) !== tracker) return;

      if (updatedMessages.length > 0) {
        if (tracker.openingPromptId === null) {
          tracker.openingPromptId =
            updatedMessages.find((message) => message.role === 'user')?.id ?? null;
        }
        tracker.messages = mergeHintMessages(
          tracker.messages,
          updatedMessages,
          tracker.lastMessageId !== null
        );
        const latestMessage = tracker.messages[tracker.messages.length - 1];
        tracker.lastMessageId = latestMessage?.id ?? tracker.lastMessageId;
      }

      const nextHint =
        deriveHint(tracker.messages, tracker.displayRoots, tracker.openingPromptId) ??
        (tracker.messages.length === 0 ? derivePipelineHint(session.pipeline) : undefined);
      if (nextHint) {
        setHint(sessionId, nextHint);
      } else {
        clearHint(sessionId);
      }
    } catch {
      // Fail-safe: keep existing static fallback labels when hint polling fails.
    }
  }

  async function pollHints() {
    if (destroyed || hintPollInFlight || hintTrackers.size === 0) return;
    hintPollInFlight = true;

    try {
      const entries = Array.from(hintTrackers.entries());
      await Promise.all(entries.map(([sessionId, tracker]) => refreshHint(sessionId, tracker)));
    } finally {
      hintPollInFlight = false;
      if (hintTrackers.size === 0) {
        stopPolling();
      }
    }
  }

  function startPolling() {
    if (destroyed || paused) return;
    if (hintPollTimer) return;
    hintPollTimer = setInterval(() => {
      void pollHints();
    }, HINT_POLL_INTERVAL_MS);
  }

  function stopPolling() {
    if (hintPollTimer) {
      clearInterval(hintPollTimer);
      hintPollTimer = null;
    }
    hintPollInFlight = false;
  }

  /** Bring the poller's trackers in line with what the view asked for. */
  function syncTrackers() {
    const activeIds = new Set([...fullIds, ...titleOnlyIds]);

    for (const sessionId of activeIds) {
      const titleOnly = !fullIds.has(sessionId);
      const tracker = hintTrackers.get(sessionId);
      if (tracker) {
        tracker.titleOnly = titleOnly;
      } else {
        hintTrackers.set(sessionId, {
          lastMessageId: null,
          messages: [],
          openingPromptId: null,
          rootKey: '',
          displayRoots: [],
          titleOnly,
        });
      }
    }

    for (const sessionId of Array.from(hintTrackers.keys())) {
      if (!activeIds.has(sessionId)) {
        hintTrackers.delete(sessionId);
        clearHint(sessionId);
        clearTitle(sessionId);
      }
    }

    if (hintTrackers.size === 0) {
      stopPolling();
    } else if (!paused) {
      startPolling();
      void pollHints();
    }
  }

  // ── From the session stream ───────────────────────────────────────────────

  const streamConsumer = sessionStream.consume(applyStreamState);

  /** Show what the stream holds for one session, by the rules the poller follows. */
  function applyStreamState(sessionId: string) {
    if (destroyed || polling || paused) return;
    const full = fullIds.has(sessionId);
    if (!full && !titleOnlyIds.has(sessionId)) return;
    const state = streamConsumer.read(sessionId);
    // Nothing has arrived for it yet, so what the row shows stays.
    if (!state) return;
    if (state.status !== 'running') {
      clearHint(sessionId);
      clearTitle(sessionId);
      return;
    }
    if (state.title) {
      setTitle(sessionId, state.title);
    } else {
      clearTitle(sessionId);
    }
    if (full && state.hint) {
      setHint(sessionId, state.hint);
    } else {
      clearHint(sessionId);
    }
  }

  /** Hand the view's sessions to the stream's owner. */
  function syncStream() {
    const activeIds = new Set([...fullIds, ...titleOnlyIds]);
    for (const sessionId of [...Object.keys(hints), ...Object.keys(titles)]) {
      if (!activeIds.has(sessionId)) {
        clearHint(sessionId);
        clearTitle(sessionId);
      }
    }
    if (paused) {
      // Off screen: let go of the sessions. The owner keeps them for a
      // while, and the hints and titles stay here so the rows don't blank.
      streamConsumer.sync({ summary: [], title: [] });
      return;
    }
    streamConsumer.sync({
      summary: [...fullIds],
      title: [...titleOnlyIds],
      displayRoots: getDisplayRootCandidates?.(),
    });
    // The owner may already hold some of them, for another view or from
    // before this one was covered.
    for (const sessionId of activeIds) applyStreamState(sessionId);
  }

  function syncSource() {
    if (polling) {
      streamConsumer.sync({ summary: [], title: [] });
      syncTrackers();
    } else {
      // The trackers go, and with them any poll still in flight. What they
      // found stays on the rows until the stream's snapshot replaces it.
      stopPolling();
      hintTrackers.clear();
      syncStream();
    }
  }

  const stopWatchingStream = sessionStream.onAvailability((availability) => {
    const next = availability === 'unavailable';
    if (destroyed || polling === next) return;
    polling = next;
    syncSource();
  });

  return {
    /**
     * Track `runningSessionIds` for hints and titles, and `titleOnlySessionIds`
     * for their ACP title alone. An id in both lists is tracked in full.
     *
     * On the stream that is its summary and its title level. When polling, a
     * title-only id costs one `getSession` per tick and no transcript fetch,
     * and a tracker's mode follows the lists on every sync: promoting an id
     * to full leaves `lastMessageId` at `null`, so its next tick fetches the
     * whole transcript.
     */
    syncRunningSessionIds(runningSessionIds: string[], titleOnlySessionIds: string[] = []) {
      if (destroyed) return;
      fullIds = new Set(runningSessionIds);
      titleOnlyIds = new Set(titleOnlySessionIds.filter((id) => !fullIds.has(id)));
      if (polling) {
        syncTrackers();
      } else {
        syncStream();
      }
    },
    /**
     * Stop updating without dropping the hints, and pick up where it left off
     * when unpaused. On the stream the sessions are let go of, which the
     * owner outlasts for a while. When polling the interval stops and the
     * trackers are kept, and unpausing polls at once.
     * `syncRunningSessionIds([])` is not a substitute: it clears the hints,
     * and when polling forgets `lastMessageId`, so every session would
     * re-fetch its whole message list on return.
     */
    setPaused(next: boolean) {
      if (destroyed || paused === next) return;
      paused = next;
      if (!polling) {
        syncStream();
      } else if (paused) {
        stopPolling();
      } else if (hintTrackers.size > 0) {
        startPolling();
        void pollHints();
      }
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
      stopWatchingStream();
      streamConsumer.release();
      stopPolling();
      hintTrackers.clear();
      if (Object.keys(hints).length > 0) {
        hints = {};
        onHintsChange(hints);
      }
      if (Object.keys(titles).length > 0) {
        titles = {};
        onTitlesChange?.(titles);
      }
    },
  };
}
