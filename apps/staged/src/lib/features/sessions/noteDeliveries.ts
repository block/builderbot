import type { SessionMessage } from '../../types';
import type { AcpTranscriptGroup, RichToolItem } from './acpTranscript';
import { noteActivityLabel } from './noteFreshness';
import { splitAtNoteIndicator } from './noteIndicators';
import { writeNoteToolCall } from './noteToolCall';

/**
 * The points in a note session's transcript where a note was delivered, which
 * the chat pane marks with the note card ("Note created", "Note updated").
 *
 * A note reaches Staged in one of two shapes (see `docs/note-delivery.md`):
 *
 * - a `write_note` call that resolved to `completed`: the tool path, which
 *   every session with the tool attached uses;
 * - an assistant message that holds the `---` note separator: the fallback
 *   for a run with no tool, and every session recorded before the tool.
 *
 * The card is drawn from the stored rows alone. Nothing in the backend records
 * "the note was updated", so this list is the one place that decides it.
 */
export interface NoteDeliveries {
  /**
   * The key of the first delivery the transcript holds, the one that created
   * the note: a tool item key, or `noteMessageKey` of an assistant message.
   * `null` when the transcript holds no delivery.
   */
  firstKey: string | null;
  /**
   * The keys of the tool items the pane shows as the note card in place of
   * their "Saved note" row.
   */
  cardToolKeys: ReadonlySet<string>;
}

export interface NoteDeliveryOptions {
  /**
   * `updatedAt` of the note the pane is opened on. Absent when the pane has
   * no note context, and then every saved call is a card.
   */
  noteUpdatedAt?: number | null;
  /**
   * The session is running. Its final turn has not reached the hooks that
   * store the note, so the stored note cannot yet be compared with the call.
   */
  sessionLive?: boolean;
}

const NO_DELIVERIES: NoteDeliveries = { firstKey: null, cardToolKeys: new Set() };

/** The delivery key of an assistant message that holds the note separator. */
export function noteMessageKey(messageId: number): string {
  return `message:${messageId}`;
}

/**
 * Lists the deliveries in the transcript groups `buildAcpTranscriptGroups`
 * produces.
 *
 * A saved call is a card, with one exception, the final-turn rule. In the
 * final turn of a session that is no longer running, and with a note context
 * to compare with, the call is a card only when the stored note is at least
 * as new as the call row. The note is stored after the turn, so a turn that
 * changed it stamps it later than its own call. A note that is older was not
 * changed by that call: the agent republished an identical note, which keeps
 * its timestamp, or the run failed before the note was stored. Such a call
 * keeps its "Saved note" row, and the pane's "Ask for the note to be updated"
 * button, which reads the same timestamp, shows under it. A turn that ends
 * with a saved call therefore shows the card or the button, not both.
 *
 * The final turn is everything after the last user message, the reading the
 * transcript builder uses for calls that never settled. Calls of earlier
 * turns are not compared: a later turn has moved the note on since.
 *
 * A call that was rejected, interrupted or is still running is never a
 * delivery and keeps its tool row.
 */
export function listNoteDeliveries(
  groups: readonly AcpTranscriptGroup[],
  options: NoteDeliveryOptions = {}
): NoteDeliveries {
  // The stored note's time, when the final turn's calls are compared with it.
  const storedAt =
    typeof options.noteUpdatedAt === 'number' && !options.sessionLive
      ? options.noteUpdatedAt
      : null;
  let finalTurnStart = -1;
  if (storedAt !== null) {
    for (let index = groups.length - 1; index >= 0; index--) {
      if (groups[index].type === 'user') {
        finalTurnStart = index;
        break;
      }
    }
  }

  let firstKey: string | null = null;
  const cardToolKeys = new Set<string>();
  for (let index = 0; index < groups.length; index++) {
    const group = groups[index];
    if (group.type === 'assistant') {
      if (messageHasNoteIndicator(group.message)) firstKey ??= noteMessageKey(group.message.id);
    } else if (group.type === 'tools') {
      for (const item of group.items) {
        if (!isSavedNoteCall(item)) continue;
        const unchangedByCall =
          storedAt !== null && index > finalTurnStart && storedAt < item.call.createdAt;
        if (unchangedByCall) continue;
        cardToolKeys.add(item.key);
        firstKey ??= item.key;
      }
    }
  }

  return firstKey === null ? NO_DELIVERIES : { firstKey, cardToolKeys };
}

/**
 * `previous` when `next` says the same thing, so a rebuild that changed no
 * delivery hands the pane the object it already has. The transcript rebuilds
 * with every stream frame, several times a second while a session streams,
 * and every tool group reads this.
 */
export function stabilizeNoteDeliveries(
  previous: NoteDeliveries | null,
  next: NoteDeliveries
): NoteDeliveries {
  if (!previous || previous === next) return next;
  if (previous.firstKey !== next.firstKey) return next;
  if (previous.cardToolKeys.size !== next.cardToolKeys.size) return next;
  for (const key of next.cardToolKeys) {
    if (!previous.cardToolKeys.has(key)) return next;
  }
  return previous;
}

/** Whether a tool item is shown as the note card. */
export function isNoteCardItem(deliveries: NoteDeliveries, item: RichToolItem): boolean {
  return deliveries.cardToolKeys.has(item.key);
}

/**
 * The label of the card shown in place of a saved call's row. A saved call is
 * never "Writing note": while the call runs, its "Saving note" row says so.
 */
export function noteCallCardLabel(
  deliveries: NoteDeliveries,
  item: RichToolItem
): ReturnType<typeof noteActivityLabel> {
  return noteActivityLabel({
    isLive: false,
    isLastGroup: false,
    isFirstNoteMessage: deliveries.firstKey === item.key,
  });
}

/** Whether an assistant message is the first delivery in the transcript. */
export function isFirstNoteDeliveryMessage(deliveries: NoteDeliveries, messageId: number): boolean {
  return deliveries.firstKey === noteMessageKey(messageId);
}

/**
 * Scans keyed on the row object. Rows keep their identity across polls except
 * the re-fetched tail, so stale entries evict themselves.
 */
const noteIndicatorCache = new WeakMap<SessionMessage, boolean>();
const writeNoteCallCache = new WeakMap<SessionMessage, boolean>();

function messageHasNoteIndicator(message: SessionMessage): boolean {
  let hasNote = noteIndicatorCache.get(message);
  if (hasNote === undefined) {
    hasNote = splitAtNoteIndicator(message.content).hasNote;
    noteIndicatorCache.set(message, hasNote);
  }
  return hasNote;
}

/**
 * Whether a tool item is a `write_note` call that resolved to `completed`.
 * The call is recognised by its tool name, not by its row label, and the
 * status is the resolved one, so a remote call completed from the outbox
 * counts as saved.
 */
function isSavedNoteCall(item: RichToolItem): boolean {
  if (item.status !== 'completed') return false;
  let isNoteCall = writeNoteCallCache.get(item.call);
  if (isNoteCall === undefined) {
    // The name is in the row's content; `rawInput` only fills in arguments.
    isNoteCall = writeNoteToolCall(item.call.content, undefined) !== null;
    writeNoteCallCache.set(item.call, isNoteCall);
  }
  return isNoteCall;
}
