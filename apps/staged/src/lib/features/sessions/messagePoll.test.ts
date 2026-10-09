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

  it('splices the tail at a stale cursor instead of duplicating rows', () => {
    // A poll read lastId=5, then an overlapping poll extended the transcript
    // to 7 before this one's fetch (which also saw 8) came back.
    const existing = [
      message(4, 'user', 'hi'),
      message(5, 'assistant', 'The'),
      message(6, 'tool_call', 'Read'),
      message(7, 'tool_result', 'ok'),
    ];
    const updated = [
      message(5, 'assistant', 'The'),
      message(6, 'tool_call', 'Read'),
      message(7, 'tool_result', 'ok'),
      message(8, 'assistant', 'Done'),
    ];
    const merged = mergeSinceMessages(existing, updated, 5);

    expect(merged!.appended).toBe(true);
    expect(merged!.messages.map((m) => m.id)).toEqual([4, 5, 6, 7, 8]);
    expect(merged!.messages[0]).toBe(existing[0]);
  });

  it('returns null when an overlapping poll already applied the same tail', () => {
    const existing = [
      message(5, 'assistant', 'The'),
      message(6, 'tool_call', 'Read'),
      message(7, 'tool_result', 'ok'),
    ];
    const updated = [
      message(5, 'assistant', 'The'),
      message(6, 'tool_call', 'Read'),
      message(7, 'tool_result', 'ok'),
    ];
    expect(mergeSinceMessages(existing, updated, 5)).toBeNull();
  });

  it('returns null when an older fetch re-covers rows a newer one already applied', () => {
    // Poll A read lastId=5 and fetched [5,6]; poll B then extended the cache to
    // 7 before A's fetch settled. A brings nothing new.
    const existing = [
      message(4, 'user', 'hi'),
      message(5, 'assistant', 'The'),
      message(6, 'tool_call', 'Read'),
      message(7, 'tool_result', 'ok'),
    ];
    const updated = [message(5, 'assistant', 'The'), message(6, 'tool_call', 'Read')];
    expect(mergeSinceMessages(existing, updated, 5)).toBeNull();
  });

  it('keeps cached rows an older fetch does not reach', () => {
    const existing = [
      message(4, 'user', 'hi'),
      message(5, 'assistant', 'The'),
      message(6, 'tool_call', 'Read'),
      message(7, 'tool_result', 'ok'),
    ];
    const changed = message(5, 'assistant', 'The search');
    const merged = mergeSinceMessages(existing, [changed, message(6, 'tool_call', 'Read')], 5);

    expect(merged!.appended).toBe(false);
    expect(merged!.messages.map((m) => m.id)).toEqual([4, 5, 6, 7]);
    expect(merged!.messages[0]).toBe(existing[0]);
    expect(merged!.messages[1]).toBe(changed);
    expect(merged!.messages[2]).not.toBe(existing[2]);
    expect(merged!.messages[3]).toBe(existing[3]);
  });

  it('does not report a stale-cursor update that ends at the cached tail as appended', () => {
    const existing = [message(5, 'assistant', 'The'), message(6, 'tool_call', 'Read')];
    const updated = [message(5, 'assistant', 'The end'), message(6, 'tool_call', 'Read')];
    const merged = mergeSinceMessages(existing, updated, 5);

    expect(merged!.appended).toBe(false);
    expect(merged!.messages.map((m) => [m.id, m.content])).toEqual([
      [5, 'The end'],
      [6, 'Read'],
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
