/**
 * noteChatNewSession.ts — Starting a new note or commit session about a note
 * from the note dialog's chat composer ("New note" / "New commit" in the
 * Send button's dropdown), instead of sending the draft into the note's own
 * session.
 */
import type {
  ComposerNewSessionDraft,
  ComposerNewSessionKind,
} from '../sessions/composerSendControl';

/** What the host starts: the finished prompt and the draft's attachments. */
export interface NoteChatSessionRequest {
  prompt: string;
  imageIds: string[];
}

/** Starts the session; resolves whether it started (or was queued). */
export type NoteChatSessionStart = (
  kind: ComposerNewSessionKind,
  request: NoteChatSessionRequest
) => Promise<boolean>;

/**
 * The new session's prompt: a reference to the note the user was composing
 * on, a blank line, then the message they typed.
 */
export function noteChatSessionPrompt(noteId: string, text: string): string {
  return `Re: #note:${noteId}\n\n${text}`;
}

export interface NoteChatSessionStarterInput {
  noteId: string | null | undefined;
  noteKind: 'branch' | 'project';
  /** The host's starter; absent where the host has no branch to start on. */
  start: NoteChatSessionStart | undefined;
  /** Closes the note dialog. */
  dismiss: () => void;
}

/**
 * The composer's new-session callback for a note, or `undefined` when the
 * actions are not offered: the host cannot start a branch session, the note
 * has no id to refer to, or it is a project note (which belongs to no single
 * repo's branch, and is referred to as `#project-note:`).
 *
 * The callback dismisses the dialog before the session starts, so the user
 * is back on the page whose timeline shows the new session.
 */
export function noteChatSessionStarter({
  noteId,
  noteKind,
  start,
  dismiss,
}: NoteChatSessionStarterInput):
  ((kind: ComposerNewSessionKind, draft: ComposerNewSessionDraft) => Promise<boolean>) | undefined {
  if (!start || !noteId || noteKind !== 'branch') return undefined;
  return (kind, draft) => {
    const request = {
      prompt: noteChatSessionPrompt(noteId, draft.text),
      imageIds: draft.imageIds,
    };
    dismiss();
    return start(kind, request);
  };
}
