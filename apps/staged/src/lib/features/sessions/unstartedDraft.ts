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

/**
 * The composer's failure line: the session did not start, and where the
 * message went. Shown beside the box that holds it again.
 */
export function unstartedDraftMessage({ reason, keptNewerText }: UnstartedDraftNotice): string {
  const failed = reason
    ? `The new session did not start (${reason}).`
    : 'The new session did not start.';
  return keptNewerText
    ? `${failed} Your message is back in the box, above what you typed since.`
    : `${failed} Your message is back in the box.`;
}

/**
 * The toast for a pane that is not on screen when the start fails: the note
 * dialog is closed by then, so the composer's failure line cannot be seen
 * until the note's chat is opened again.
 */
export function unstartedDraftToast({ reason, keptNewerText }: UnstartedDraftNotice): {
  title: string;
  description: string;
} {
  const where = keptNewerText
    ? "Your message is back in the note's chat box, above what you typed since."
    : "Your message is back in the note's chat box.";
  return {
    title: 'The new session did not start',
    description: reason ? `${reason}. ${where}` : where,
  };
}
