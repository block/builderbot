import * as commands from '../../api/commands';
import type { BranchTimeline, SessionMessage } from '../../types';
import { formatToolDisplay } from '../sessions/sessionModalHelpers';

const HINT_POLL_INTERVAL_MS = 750;
const MAX_HINT_LENGTH = 72;
const MAX_HINT_MESSAGES = 40;

type HintTracker = {
  lastMessageId: number | null;
  messages: SessionMessage[];
};

export type PendingHintItemType = 'pending-commit' | 'generating-note' | 'generating-review';

export type PendingHintItem = {
  type: PendingHintItemType;
  sessionId?: string;
};

/** Strip XML-tagged context blocks (action, branch-history) from display text. */
function stripXmlTags(text: string): string {
  return text.replace(/<(action|branch-history)>[\s\S]*?<\/\1>/g, '').trim();
}

function normalizeHintText(text: string): string | undefined {
  const cleaned = stripXmlTags(text)
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/<\/?[^>]+>/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
  if (!cleaned) return undefined;
  if (cleaned.length <= MAX_HINT_LENGTH) return cleaned;
  return `${cleaned.slice(0, MAX_HINT_LENGTH - 1).trimEnd()}…`;
}

function withTrailingEllipsis(text: string): string {
  if (/[.!?…]$/.test(text)) {
    if (text.endsWith('.')) {
      return `${text.slice(0, -1)}…`;
    }
    return text;
  }
  return `${text}…`;
}

function formatToolCallHint(content: string, repoDir?: string | null): string | undefined {
  const { verb, detail } = formatToolDisplay(content, repoDir, true);
  const text = detail ? `${verb} ${detail}` : verb;
  if (!text) return undefined;
  return normalizeHintText(withTrailingEllipsis(text));
}

function formatAssistantHint(content: string): string | undefined {
  const stripped = stripXmlTags(content);

  // When the response contains review fenced blocks, the surrounding text is just
  // LLM preamble (e.g. "I now have a complete picture…") — not a useful hint.
  if (/```review-(?:title|comments)/.test(stripped)) return undefined;

  const lines = stripped
    .replace(/```[\s\S]*?```/g, ' ')
    .split(/\r?\n/)
    .map((line) =>
      line
        .trim()
        .replace(/^[-*]\s+/, '')
        .replace(/^\d+\.\s+/, '')
        .replace(/^#+\s+/, '')
    )
    .filter(Boolean);

  for (let i = lines.length - 1; i >= 0; i--) {
    const normalized = normalizeHintText(lines[i]);
    if (normalized) {
      return normalized;
    }
  }

  return undefined;
}

function deriveHint(messages: SessionMessage[], repoDir?: string | null): string | undefined {
  let latestToolHint: { id: number; text: string } | null = null;
  let latestAssistantHint: { id: number; text: string } | null = null;

  for (let i = messages.length - 1; i >= 0; i--) {
    const message = messages[i];
    if (!latestToolHint && message.role === 'tool_call') {
      const hint = formatToolCallHint(message.content, repoDir);
      if (hint) {
        latestToolHint = { id: message.id, text: hint };
      }
    }
    if (!latestAssistantHint && message.role === 'assistant') {
      const hint = formatAssistantHint(message.content);
      if (hint) {
        latestAssistantHint = { id: message.id, text: hint };
      }
    }
    if (latestToolHint && latestAssistantHint) {
      break;
    }
  }

  if (latestToolHint && (!latestAssistantHint || latestToolHint.id >= latestAssistantHint.id)) {
    return latestToolHint.text;
  }
  if (latestAssistantHint) {
    return latestAssistantHint.text;
  }
  if (latestToolHint) {
    return latestToolHint.text;
  }
  return undefined;
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
  getRepoDir?: () => string | null | undefined
) {
  const hintTrackers = new Map<string, HintTracker>();
  let hints: Record<string, string> = {};
  let hintPollTimer: ReturnType<typeof setInterval> | null = null;
  let hintPollInFlight = false;
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

  async function refreshHint(sessionId: string, tracker: HintTracker) {
    if (destroyed) return;
    try {
      const session = await commands.getSession(sessionId);
      if (destroyed) return;
      if (!session || session.status !== 'running') {
        hintTrackers.delete(sessionId);
        clearHint(sessionId);
        return;
      }

      const updatedMessages =
        tracker.lastMessageId === null
          ? await commands.getSessionMessages(sessionId)
          : await commands.getSessionMessagesSince(sessionId, tracker.lastMessageId);

      if (destroyed || !hintTrackers.has(sessionId)) return;

      if (updatedMessages.length > 0) {
        tracker.messages = mergeHintMessages(
          tracker.messages,
          updatedMessages,
          tracker.lastMessageId !== null
        );
        const latestMessage = tracker.messages[tracker.messages.length - 1];
        tracker.lastMessageId = latestMessage?.id ?? tracker.lastMessageId;
      }

      const nextHint = deriveHint(tracker.messages, getRepoDir?.());
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
    if (destroyed) return;
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
    syncRunningSessionIds(runningSessionIds: string[]) {
      if (destroyed) return;
      const activeIds = new Set(runningSessionIds);

      for (const sessionId of activeIds) {
        if (!hintTrackers.has(sessionId)) {
          hintTrackers.set(sessionId, {
            lastMessageId: null,
            messages: [],
          });
        }
      }

      for (const sessionId of Array.from(hintTrackers.keys())) {
        if (!activeIds.has(sessionId)) {
          hintTrackers.delete(sessionId);
          clearHint(sessionId);
        }
      }

      if (hintTrackers.size > 0) {
        startPolling();
        void pollHints();
      } else {
        stopPolling();
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
    },
  };
}
