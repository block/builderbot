/**
 * unstartedDraft.ts — Putting a draft back after the new session it was
 * handed to did not start (the composer's "New note" / "New commit").
 *
 * The draft leaves the box before the host starts the session, and the host
 * closes the dialog as it does. A start can take a while, so by the time it
 * fails the user may have reopened the note and typed something else. The
 * returned message therefore never replaces what is there: it goes back
 * above it.
 */

/** What separates the returned message from the text typed since. */
const RESTORED_DRAFT_SEPARATOR = '\n\n';

export interface RestoredDraft {
  /** What the composer should hold now. */
  text: string;
  /** The returned message was put above text typed since. */
  keptNewerText: boolean;
}

/**
 * The draft to hold once `original` (the message whose session did not
 * start) is put back into a composer that now holds `current`. Nothing
 * typed since is dropped: the message goes above it, a blank line apart.
 * A composer that is empty, or that already holds the same message again,
 * needs no joining.
 */
export function restoreUnstartedDraft(original: string, current: string): RestoredDraft {
  if (current.trim() === '') return { text: original, keptNewerText: false };
  if (current.trim() === original.trim()) return { text: current, keptNewerText: false };
  return {
    text: `${original}${RESTORED_DRAFT_SEPARATOR}${current.trimStart()}`,
    keptNewerText: true,
  };
}

/**
 * The attachments to hold once the returned message's are put back beside
 * any attached since: the returned ones first, as in the text, each once.
 */
export function restoreUnstartedAttachments(original: string[], current: string[]): string[] {
  const restored = new Set(original);
  return [...original, ...current.filter((id) => !restored.has(id))];
}

export interface UnstartedDraftNotice {
  /** Why the start failed, when the host said (a thrown error's message). */
  reason?: string | null;
  /** The returned message sits above text typed since. */
  keptNewerText: boolean;
}

/** `reason` without the full stop the notice supplies itself, or `null`. */
function bareReason(reason: string | null | undefined): string | null {
  return reason?.trim().replace(/\.+$/, '').trimEnd() || null;
}

/**
 * The composer's failure line: the session did not start, and where the
 * message went. Shown beside the box that holds it again.
 */
export function unstartedDraftMessage({ reason, keptNewerText }: UnstartedDraftNotice): string {
  const why = bareReason(reason);
  const failed = why ? `The new session did not start (${why}).` : 'The new session did not start.';
  return keptNewerText
    ? `${failed} Your message is back in the box, above what you typed since.`
    : `${failed} Your message is back in the box.`;
}

export interface UnstartedDraftToastNotice extends UnstartedDraftNotice {
  /**
   * The pane that held the message is gone (unmounted, or showing another
   * session), so only the stored draft could be written: no box is known to
   * hold the message, and the toast must not say one does.
   */
  boxGone?: boolean;
  /** The message's attachments went with that pane. */
  attachmentsLost?: boolean;
}

/**
 * The toast for a pane that is not on screen when the start fails: the note
 * dialog is closed by then, so the composer's failure line cannot be seen
 * until the note's chat is opened again.
 */
export function unstartedDraftToast({
  reason,
  keptNewerText,
  boxGone = false,
  attachmentsLost = false,
}: UnstartedDraftToastNotice): {
  title: string;
  description: string;
} {
  const above = keptNewerText ? ', above what you typed since' : '';
  let where = boxGone
    ? `Your message was saved as the note's chat draft${above}. Copy it from here if the chat does not show it.`
    : `Your message is back in the note's chat box${above}.`;
  if (boxGone && attachmentsLost) where += ' Its attachments were not kept.';
  const why = bareReason(reason);
  return {
    title: 'The new session did not start',
    // A reason that ends its own sentence ("Not allowed!") gets no full stop.
    description: why ? `${why}${/[!?]$/.test(why) ? '' : '.'} ${where}` : where,
  };
}

/**
 * Takes a returned draft into a composer: the pane's own put-back, with the
 * session, the message, its attachments and why the start failed.
 */
export type UnstartedDraftBox = (
  sessionId: string,
  text: string,
  imageIds: string[],
  reason: string | null
) => Promise<void>;

/** The composers on screen right now, by the session they show. */
const openBoxes = new Map<string, Set<UnstartedDraftBox>>();

/**
 * A pane has `sessionId`'s composer on screen: a draft returned to that
 * session by a pane that no longer shows it can go into this box. Returns
 * the withdrawal, for when the pane leaves the screen.
 */
export function offerUnstartedDraftBox(sessionId: string, box: UnstartedDraftBox): () => void {
  const boxes = openBoxes.get(sessionId) ?? new Set<UnstartedDraftBox>();
  openBoxes.set(sessionId, boxes);
  boxes.add(box);
  return () => {
    boxes.delete(box);
    if (boxes.size === 0 && openBoxes.get(sessionId) === boxes) openBoxes.delete(sessionId);
  };
}

/** A composer showing `sessionId` on screen that is not `own`, if any. */
export function otherUnstartedDraftBox(
  sessionId: string,
  own: UnstartedDraftBox
): UnstartedDraftBox | null {
  for (const box of openBoxes.get(sessionId) ?? []) {
    if (box !== own) return box;
  }
  return null;
}
