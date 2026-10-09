import { describe, expect, it } from 'vitest';
import { mergeSinceMessages, openAssistantRefetchIds } from './messagePoll';
import type { MessageRole, SessionMessage } from '../../types';

function message(id: number, role: MessageRole, content: string): SessionMessage {
  return { id, sessionId: 's1', role, content, createdAt: id };
}

describe('openAssistantRefetchIds', () => {
  it('asks for the latest assistant row once tool rows have pushed it off the tail', () => {
    const messages = [
      message(1, 'user', 'hi'),
      message(2, 'assistant', 'The'),
      message(3, 'tool_call', 'Read'),
      message(4, 'tool_result', 'ok'),
    ];
    expect(openAssistantRefetchIds(messages)).toEqual([2]);
  });

  it('asks for nothing when the assistant row is the tail', () => {
    const messages = [message(1, 'user', 'hi'), message(2, 'assistant', 'The')];
    expect(openAssistantRefetchIds(messages)).toEqual([]);
  });

  it('asks for nothing when there is no assistant row', () => {
    expect(openAssistantRefetchIds([])).toEqual([]);
    expect(openAssistantRefetchIds([message(1, 'user', 'hi')])).toEqual([]);
  });

  it('only asks for the latest assistant row', () => {
    const messages = [
      message(1, 'assistant', 'first'),
      message(2, 'tool_call', 'Read'),
      message(3, 'assistant', 'second'),
      message(4, 'tool_call', 'Grep'),
    ];
    expect(openAssistantRefetchIds(messages)).toEqual([3]);
  });
});

describe('mergeSinceMessages', () => {
  it('replaces an open assistant row behind the tail and keeps the rest', () => {
    const existing = [
      message(1, 'user', 'hi'),
      message(2, 'assistant', 'The'),
      message(3, 'tool_call', 'Read'),
      message(4, 'tool_result', 'ok'),
    ];
    const resumed = message(2, 'assistant', 'The search is back');
    const merged = mergeSinceMessages(existing, [resumed, message(4, 'tool_result', 'ok')], 4);

    expect(merged).not.toBeNull();
    expect(merged!.appended).toBe(false);
    expect(merged!.messages).not.toBe(existing);
    expect(merged!.messages[1]).toBe(resumed);
    expect(merged!.messages[0]).toBe(existing[0]);
    expect(merged!.messages[2]).toBe(existing[2]);
    expect(merged!.messages[3]).toBe(existing[3]);
  });

  it('returns null when the refetched row and the tail are unchanged', () => {
    const existing = [
      message(2, 'assistant', 'The'),
      message(3, 'tool_call', 'Read'),
      message(4, 'tool_result', 'ok'),
    ];
    const updated = [message(2, 'assistant', 'The'), message(4, 'tool_result', 'ok')];
    expect(mergeSinceMessages(existing, updated, 4)).toBeNull();
  });

  it('returns null when nothing came back', () => {
    expect(mergeSinceMessages([message(1, 'assistant', 'a')], [], 1)).toBeNull();
  });

  it('replaces the tail in place while it streams', () => {
    const existing = [message(1, 'user', 'hi'), message(2, 'assistant', 'The')];
    const merged = mergeSinceMessages(existing, [message(2, 'assistant', 'The end')], 2);

    expect(merged!.appended).toBe(false);
    expect(merged!.messages.map((m) => m.content)).toEqual(['hi', 'The end']);
    expect(merged!.messages[0]).toBe(existing[0]);
  });

  it('reports rows past the previous tail as appended', () => {
    const existing = [message(2, 'assistant', 'The'), message(3, 'tool_call', 'Read')];
    const updated = [
      message(2, 'assistant', 'The search'),
      message(3, 'tool_call', 'Read'),
      message(4, 'tool_result', 'ok'),
    ];
    const merged = mergeSinceMessages(existing, updated, 3);

    expect(merged!.appended).toBe(true);
    expect(merged!.messages.map((m) => [m.id, m.content])).toEqual([
      [2, 'The search'],
      [3, 'Read'],
      [4, 'ok'],
    ]);
  });

  it('notices metadata-only changes to a refetched row', () => {
    const existing = [message(2, 'assistant', 'The'), message(3, 'tool_call', 'Read')];
    const updated = [
      { ...message(2, 'assistant', 'The'), acpMessageId: 'msg-1' },
      message(3, 'tool_call', 'Read'),
    ];
    const merged = mergeSinceMessages(existing, updated, 3);

    expect(merged!.messages[0].acpMessageId).toBe('msg-1');
    expect(merged!.messages[1]).toBe(existing[1]);
  });
});
