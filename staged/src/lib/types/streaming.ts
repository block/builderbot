import { parseAssistantContent, type Message, type ContentSegment } from '../services/ai';

/** A segment for display - text or tool call */
export type DisplaySegment =
  | { type: 'text'; text: string }
  | { type: 'tool'; id: string; title: string; status: string };

/** Display message - user has plain text, assistant has segments */
export interface DisplayMessage {
  role: 'user' | 'assistant';
  /** For user: plain text. For assistant: unused (use segments) */
  content: string;
  /** For assistant: ordered segments. For user: empty */
  segments: DisplaySegment[];
}

/** Convert a persisted Message to a DisplayMessage */
export function toDisplayMessage(msg: Message): DisplayMessage {
  if (msg.role === 'user') {
    return { role: 'user', content: msg.content, segments: [] };
  }
  const contentSegments = parseAssistantContent(msg.content);
  const segments: DisplaySegment[] = contentSegments.map((seg: ContentSegment) => {
    if (seg.type === 'text') {
      return { type: 'text' as const, text: seg.text };
    } else {
      return {
        type: 'tool' as const,
        id: seg.id,
        title: seg.title,
        status: seg.status,
      };
    }
  });
  return { role: 'assistant', content: '', segments };
}
