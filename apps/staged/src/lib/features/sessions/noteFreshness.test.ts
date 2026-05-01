import { describe, expect, it } from 'vitest';
import type { Session, SessionMessage, SessionStatus, CompletionReason } from '../../types';
import {
  buildNoteFollowupMessage,
  countAssistantMessagesAfterNote,
  formatChatButtonLabel,
  getNoteFollowupLabel,
  hasNoteFollowupBeenSent,
  latestAssistantMessage,
  type LinkedNoteContext,
} from './noteFreshness';

function session(
  status: SessionStatus = 'completed',
  completionReason: CompletionReason | null = 'turn_complete'
): Session {
  return {
    id: 'session-1',
    prompt: 'Write a note',
    status,
    agentId: null,
    errorMessage: null,
    completionReason,
    createdAt: 1000,
    updatedAt: 2000,
  };
}

function message(
  id: number,
  role: SessionMessage['role'],
  createdAt: number,
  content = `${role} ${id}`
): SessionMessage {
  return {
    id,
    sessionId: 'session-1',
    role,
    content,
    createdAt,
  };
}

function note(overrides: Partial<LinkedNoteContext> = {}): LinkedNoteContext {
  return {
    id: 'note-1',
    title: 'Note',
    content: '# Note\n\nBody',
    updatedAt: 2000,
    hasParsedNote: true,
    ...overrides,
  };
}

describe('note freshness', () => {
  it('does not show a note follow-up CTA without a linked note', () => {
    const messages = [message(1, 'assistant', 3000)];

    expect(getNoteFollowupLabel(session(), messages, null)).toBeNull();
  });

  it.each([
    ['queued', null],
    ['running', null],
    ['cancelled', 'interrupted'],
    ['error', 'crashed'],
    ['completed', 'interrupted'],
  ] satisfies [SessionStatus, CompletionReason | null][])(
    'does not show a note follow-up CTA for %s sessions with %s completion',
    (status, completionReason) => {
      const messages = [message(1, 'assistant', 3000)];

      expect(getNoteFollowupLabel(session(status, completionReason), messages, note())).toBeNull();
    }
  );

  it('asks for a note to be written when an empty linked note has a later assistant message', () => {
    const messages = [message(1, 'assistant', 3000)];
    const emptyNote = note({ content: '', hasParsedNote: false });

    expect(getNoteFollowupLabel(session(), messages, emptyNote)).toBe(
      'Ask for a note to be written'
    );
  });

  it('asks for the note to be updated when a parsed linked note has a later assistant message', () => {
    const messages = [message(1, 'assistant', 3000)];

    expect(getNoteFollowupLabel(session(), messages, note())).toBe(
      'Ask for the note to be updated'
    );
  });

  it('ignores assistant messages before the note updated timestamp', () => {
    const messages = [
      message(1, 'assistant', 1500),
      message(2, 'user', 3000),
      message(3, 'tool_result', 3500),
    ];

    expect(countAssistantMessagesAfterNote(messages, 2000)).toBe(0);
    expect(getNoteFollowupLabel(session(), messages, note())).toBeNull();
  });

  it('counts multiple assistant messages after the note updated timestamp', () => {
    const messages = [
      message(1, 'assistant', 1500),
      message(2, 'assistant', 2500),
      message(3, 'user', 3000),
      message(4, 'assistant', 3500),
    ];

    expect(countAssistantMessagesAfterNote(messages, 2000)).toBe(2);
  });

  it('uses the newest assistant message by timestamp and id', () => {
    const messages = [
      message(1, 'assistant', 3000),
      message(2, 'assistant', 2500),
      message(3, 'assistant', 3000),
    ];

    expect(latestAssistantMessage(messages)?.id).toBe(3);
  });

  it('builds a structured follow-up prompt with a readable visible request', () => {
    const updateMessage = buildNoteFollowupMessage(true);
    const writeMessage = buildNoteFollowupMessage(false);

    expect(updateMessage.startsWith('<action>')).toBe(true);
    expect(updateMessage).toContain('```suggested-next-steps');
    expect(updateMessage).toContain('\n---\n# <Title>');
    expect(updateMessage).toContain('Please update the note to reflect the latest chat.');
    expect(writeMessage).toContain('Please write the note for this session.');
  });

  it('suppresses note followup CTA when a followup was already sent', () => {
    const followupContent = buildNoteFollowupMessage(true);
    const messages = [
      message(1, 'assistant', 1500),
      message(2, 'user', 2500, followupContent),
      message(3, 'assistant', 3000),
    ];

    expect(hasNoteFollowupBeenSent(messages)).toBe(true);
    expect(getNoteFollowupLabel(session(), messages, note())).toBeNull();
  });

  it('does not suppress CTA when no followup has been sent', () => {
    const messages = [message(1, 'user', 1500, 'Can you help me?'), message(2, 'assistant', 3000)];

    expect(hasNoteFollowupBeenSent(messages)).toBe(false);
    expect(getNoteFollowupLabel(session(), messages, note())).toBe(
      'Ask for the note to be updated'
    );
  });
});

describe('formatChatButtonLabel', () => {
  it('returns singular label for 1 message', () => {
    expect(formatChatButtonLabel(1)).toBe('1 message after note in chat');
  });

  it('returns plural label for multiple messages', () => {
    expect(formatChatButtonLabel(5)).toBe('5 messages after note in chat');
  });

  it('returns generic label for 0 messages', () => {
    expect(formatChatButtonLabel(0)).toBe('View chat');
  });
});
