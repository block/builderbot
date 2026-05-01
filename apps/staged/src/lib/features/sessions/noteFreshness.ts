import type { Session, SessionMessage } from '../../types';

export interface LinkedNoteContext {
  id: string;
  title: string;
  content: string;
  updatedAt: number;
  hasParsedNote: boolean;
}

/** Info passed when a note is clicked in the timeline. */
export interface NoteClickInfo {
  noteId: string;
  title: string;
  content: string;
  sessionId?: string;
  updatedAt?: number;
}

export function countAssistantMessagesAfterNote(
  messages: SessionMessage[],
  noteUpdatedAt: number | null | undefined
): number {
  if (typeof noteUpdatedAt !== 'number') return 0;
  return messages.filter(
    (message) => message.role === 'assistant' && message.createdAt > noteUpdatedAt
  ).length;
}

/**
 * Formats a label for the "view chat" button in the note modal.
 * Shows the count of assistant messages after the note was last updated.
 */
export function formatChatButtonLabel(messagesAfterNote: number): string {
  if (messagesAfterNote === 1) return '1 message after note in chat';
  if (messagesAfterNote > 1) return `${messagesAfterNote} messages after note in chat`;
  return 'View chat';
}

export function latestAssistantMessage(messages: SessionMessage[]): SessionMessage | null {
  let latest: SessionMessage | null = null;
  for (const message of messages) {
    if (message.role !== 'assistant') continue;
    if (
      !latest ||
      message.createdAt > latest.createdAt ||
      (message.createdAt === latest.createdAt && message.id > latest.id)
    ) {
      latest = message;
    }
  }
  return latest;
}

/** Marker text embedded in the note followup action block. */
const NOTE_FOLLOWUP_MARKER = 'The user is asking you to';

/**
 * Returns true if any user message in the session already contains the
 * note-followup prompt. Used to suppress the CTA after it has been clicked.
 */
export function hasNoteFollowupBeenSent(messages: SessionMessage[]): boolean {
  return messages.some((m) => m.role === 'user' && m.content.includes(NOTE_FOLLOWUP_MARKER));
}

export function shouldAskForNoteUpdate(
  session: Session | null,
  messages: SessionMessage[],
  noteContext: LinkedNoteContext | null | undefined
): boolean {
  if (!session || !noteContext) return false;
  if (session.status !== 'completed' || session.completionReason !== 'turn_complete') return false;
  if (hasNoteFollowupBeenSent(messages)) return false;

  const latestAssistant = latestAssistantMessage(messages);
  return !!latestAssistant && latestAssistant.createdAt > noteContext.updatedAt;
}

export function getNoteFollowupLabel(
  session: Session | null,
  messages: SessionMessage[],
  noteContext: LinkedNoteContext | null | undefined
): string | null {
  if (!shouldAskForNoteUpdate(session, messages, noteContext)) return null;
  return noteContext?.hasParsedNote
    ? 'Ask for the note to be updated'
    : 'Ask for a note to be written';
}

export function buildNoteFollowupMessage(hasParsedNote: boolean): string {
  const visibleRequest = hasParsedNote
    ? 'Please update the note to reflect the latest chat.'
    : 'Please write the note for this session.';

  return `<action>
The user is asking you to ${hasParsedNote ? 'update the linked note' : 'write the linked note'} from the latest chat history.

Use the existing conversation context. Do not create commits.

Your final response must include a suggested-next-steps fenced block followed by the note content after a horizontal rule:

\`\`\`suggested-next-steps
{"suggestedNextCommitStep": null, "suggestedNextNoteStep": null}
\`\`\`

---
# <Title>
<Body>

Formatting requirements:
- The opening fence line for suggested-next-steps must be exactly: \`\`\`suggested-next-steps
- The closing fence line must be exactly: \`\`\`
- Put only a JSON object inside the suggested-next-steps block.
- Include both nullable string fields: suggestedNextCommitStep and suggestedNextNoteStep.
- Keep suggested next steps concise; use null when there is no clear next action.
- The \`---\` separator must be on its own line.
- The note content must start immediately after \`---\` with a markdown H1.
- Do not wrap the note in code fences.
</action>

${visibleRequest}`;
}
