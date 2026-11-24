import * as commands from '../../api/commands';
import type { BranchTimeline, SessionMessage } from '../../types';
import {
  displayRootKey,
  normalizeDisplayRoots,
  resolveDisplayRoots,
  type DisplayRootInput,
} from '../sessions/pathDisplayRoots';
import { stripXmlTags } from '../sessions/sessionModalHelpers';
import { deriveHint, derivePipelineHint } from './sessionHint';

const HINT_POLL_INTERVAL_MS = 750;
const MAX_HINT_MESSAGES = 40;

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

function normalizeTitleText(title: string | null | undefined): string | undefined {
  if (!title) return undefined;
  const cleaned = stripXmlTags(title).replace(/\s+/g, ' ').trim();
  return cleaned || undefined;
}

function mergeHintMessages(
  previous: SessionMessage[],
  updated: SessionMessage[],
  hadPrevious: boolean
): SessionMessage[] {
  if (updated.length === 0) return previous;
  const merged = hadPrevious ? [...previous.slice(0, -1), ...updated] : updated;
  return merged.slice(-MAX_HINT_MESSAGES);
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
      if (destroyed) return;
      if (!session || session.status !== 'running') {
        hintTrackers.delete(sessionId);
        clearHint(sessionId);
        clearTitle(sessionId);
        return;
      }

      const acpTitle = normalizeTitleText(session.acpTitle);
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
          if (destroyed || !hintTrackers.has(sessionId) || tracker.rootKey !== nextRootKey) return;
          tracker.displayRoots = resolvedRoots;
        }
      }

      const updatedMessages =
        tracker.lastMessageId === null
          ? (await commands.getSessionMessages(sessionId)).data
          : await commands.getSessionMessagesSince(sessionId, tracker.lastMessageId);

      if (destroyed || !hintTrackers.has(sessionId)) return;

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

  return {
    /**
     * Track `runningSessionIds` for hints and titles, and `titleOnlySessionIds`
     * for their ACP title alone (one `getSession` per tick, no transcript
     * fetch). An id in both lists is tracked in full. A tracker's mode follows
     * the lists on every sync: promoting an id to full leaves `lastMessageId`
     * at `null`, so its next tick fetches the whole transcript.
     */
    syncRunningSessionIds(runningSessionIds: string[], titleOnlySessionIds: string[] = []) {
      if (destroyed) return;
      const fullIds = new Set(runningSessionIds);
      const activeIds = new Set([...runningSessionIds, ...titleOnlySessionIds]);

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
    },
    /**
     * Stop the interval without dropping trackers or hints, and pick up where
     * it left off (polling at once) when unpaused. `syncRunningSessionIds([])`
     * is not a substitute: it clears the hints and forgets `lastMessageId`,
     * so every session would re-fetch its whole message list on return.
     */
    setPaused(next: boolean) {
      if (destroyed || paused === next) return;
      paused = next;
      if (paused) {
        stopPolling();
      } else if (hintTrackers.size > 0) {
        startPolling();
        void pollHints();
      }
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
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
