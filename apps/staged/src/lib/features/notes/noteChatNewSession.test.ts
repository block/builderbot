import { describe, expect, it, vi } from 'vitest';
import {
  noteChatSessionPrompt,
  noteChatSessionStarter,
  type NoteChatSessionStart,
} from './noteChatNewSession';

describe('noteChatSessionPrompt', () => {
  it('refers to the note, then a blank line, then the typed message', () => {
    expect(noteChatSessionPrompt('abc123', 'Split this into two PRs')).toBe(
      'Re: #note:abc123\n\nSplit this into two PRs'
    );
  });

  it('keeps the message as typed, line breaks and references included', () => {
    const text = 'First line\n\nsee #commit:deadbee\n  indented';
    expect(noteChatSessionPrompt('n1', text)).toBe(`Re: #note:n1\n\n${text}`);
  });
});

describe('noteChatSessionStarter', () => {
  const start = (result = true) => vi.fn<NoteChatSessionStart>(async () => result);

  it('is not offered without a host starter', () => {
    expect(
      noteChatSessionStarter({
        noteId: 'n1',
        noteKind: 'branch',
        start: undefined,
        dismiss: vi.fn(),
      })
    ).toBeUndefined();
  });

  it('is not offered for a note with no id to refer to', () => {
    for (const noteId of [null, undefined, '']) {
      expect(
        noteChatSessionStarter({ noteId, noteKind: 'branch', start: start(), dismiss: vi.fn() })
      ).toBeUndefined();
    }
  });

  it('is not offered for project notes, even when the host could start a session', () => {
    expect(
      noteChatSessionStarter({
        noteId: 'n1',
        noteKind: 'project',
        start: start(),
        dismiss: vi.fn(),
      })
    ).toBeUndefined();
  });

  it('dismisses the dialog, then starts the chosen kind with the note prompt and attachments', async () => {
    const order: string[] = [];
    const hostStart = vi.fn<NoteChatSessionStart>(async () => {
      order.push('start');
      return true;
    });
    const dismiss = vi.fn(() => {
      order.push('dismiss');
    });
    const starter = noteChatSessionStarter({
      noteId: 'n1',
      noteKind: 'branch',
      start: hostStart,
      dismiss,
    });

    await expect(starter!('commit', { text: 'Do it', imageIds: ['img-1', 'img-2'] })).resolves.toBe(
      true
    );
    expect(order).toEqual(['dismiss', 'start']);
    expect(dismiss).toHaveBeenCalledTimes(1);
    expect(hostStart).toHaveBeenCalledWith('commit', {
      prompt: 'Re: #note:n1\n\nDo it',
      imageIds: ['img-1', 'img-2'],
    });
  });

  it('reports a start that failed, with the dialog already dismissed', async () => {
    const dismiss = vi.fn();
    const starter = noteChatSessionStarter({
      noteId: 'n1',
      noteKind: 'branch',
      start: start(false),
      dismiss,
    });

    await expect(starter!('note', { text: 'Why?', imageIds: [] })).resolves.toBe(false);
    expect(dismiss).toHaveBeenCalledTimes(1);
  });
});
