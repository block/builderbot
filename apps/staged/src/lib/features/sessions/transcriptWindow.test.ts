import { describe, expect, it } from 'vitest';
import type { MessageRole, SessionMessage } from '../../types';
import {
  applyLatestWindow,
  metadataRowsForWindow,
  pageRequestLimit,
  prependOlderMessages,
  transcriptPageFromRows,
} from './transcriptWindow';

function message(
  id: number,
  role: MessageRole = 'assistant',
  extra: Partial<SessionMessage> = {}
): SessionMessage {
  return { id, sessionId: 's1', role, content: `m${id}`, createdAt: id, ...extra };
}

const ids = (messages: readonly SessionMessage[]) => messages.map((m) => m.id);

describe('transcriptPageFromRows', () => {
  it('drops the over-fetched oldest row and reports more history', () => {
    const rows = [message(5), message(6), message(7), message(8)];
    const page = transcriptPageFromRows(rows, 3);
    expect(ids(page.messages)).toEqual([6, 7, 8]);
    expect(page.hasMore).toBe(true);
  });

  it('keeps a short response whole and reports the start of history', () => {
    expect(transcriptPageFromRows([message(1), message(2)], 3)).toEqual({
      messages: [message(1), message(2)],
      hasMore: false,
    });
    expect(transcriptPageFromRows([message(1), message(2), message(3)], 3).hasMore).toBe(false);
    expect(transcriptPageFromRows([], 3)).toEqual({ messages: [], hasMore: false });
  });

  it('requests one row beyond the page it shows', () => {
    expect(pageRequestLimit(300)).toBe(301);
  });
});

describe('prependOlderMessages', () => {
  it('puts the older page in front, keeping ids ascending', () => {
    const existing = [message(10), message(11)];
    const older = [message(7), message(8), message(9)];
    expect(ids(prependOlderMessages(existing, older))).toEqual([7, 8, 9, 10, 11]);
  });

  it('skips rows the pane already holds', () => {
    const existing = [message(9), message(10)];
    const older = [message(8), message(9), message(10)];
    expect(ids(prependOlderMessages(existing, older))).toEqual([8, 9, 10]);
  });

  it('is a copy when the page adds nothing', () => {
    const existing = [message(9)];
    const result = prependOlderMessages(existing, [message(9)]);
    expect(result).toEqual(existing);
    expect(result).not.toBe(existing);
    expect(ids(prependOlderMessages([], [message(1)]))).toEqual([1]);
  });
});

describe('applyLatestWindow', () => {
  it('replaces the window when no older page has been loaded', () => {
    const current = { messages: [message(5), message(6)], hasMore: true };
    const fresh = {
      messages: [message(5, 'assistant', { content: 'grown' }), message(6)],
      hasMore: false,
    };
    expect(applyLatestWindow(current, fresh)).toEqual(fresh);
  });

  it('keeps loaded older pages and their verdict on more history', () => {
    const current = {
      messages: [message(1), message(2), message(3), message(4)],
      hasMore: false,
    };
    const fresh = { messages: [message(3), message(4), message(5)], hasMore: true };
    const result = applyLatestWindow(current, fresh);
    expect(ids(result.messages)).toEqual([1, 2, 3, 4, 5]);
    expect(result.hasMore).toBe(false);
  });

  it('keeps rows the poll appended past the fresh window', () => {
    const current = { messages: [message(5), message(6), message(7)], hasMore: true };
    const fresh = { messages: [message(5), message(6)], hasMore: true };
    expect(ids(applyLatestWindow(current, fresh).messages)).toEqual([5, 6, 7]);
  });

  it('adopts an empty fresh window as-is', () => {
    const current = { messages: [message(1)], hasMore: false };
    expect(applyLatestWindow(current, { messages: [], hasMore: false })).toEqual({
      messages: [],
      hasMore: false,
    });
  });
});

describe('metadataRowsForWindow', () => {
  const toolRow = (id: number, toolCallId: string, extra: Partial<SessionMessage> = {}) =>
    message(id, 'tool_call', {
      content: '',
      acpEventKind: 'tool_call',
      acpToolCallId: toolCallId,
      acpToolStatus: 'in_progress',
      ...extra,
    });

  it('passes everything through when the whole transcript is loaded', () => {
    const metadata = [toolRow(1, 'tc-1'), message(2, 'assistant', { acpEventKind: 'plan_update' })];
    const visible = [message(5, 'user')];
    expect(metadataRowsForWindow(metadata, visible, false)).toEqual(metadata);
    expect(metadataRowsForWindow(metadata, [], true)).toEqual(metadata);
  });

  it('drops rows older than the window unless they belong to a loaded tool call', () => {
    const visible = [
      message(10, 'tool_call', { content: 'Read', acpToolCallId: 'tc-boundary' }),
      message(11, 'tool_result', { content: 'ok' }),
      message(12, 'assistant'),
    ];
    const metadata = [
      // Orphan: its visible call is above the window; would render as an
      // in-progress card at the top.
      toolRow(3, 'tc-old'),
      // An in-flight *visible* row the metadata endpoint still returns.
      message(4, 'tool_call', {
        content: 'Run tests',
        acpEventKind: 'tool_call',
        acpToolCallId: 'tc-old-visible',
        acpToolStatus: 'in_progress',
      }),
      // An announcement for a tool above the window, which would otherwise
      // pair with the earliest loaded pikchr tool.
      message(5, 'assistant', { content: '', acpEventKind: 'pikchr_session_started' }),
      // Raced ahead of its visible row at the boundary: keep, it carries the
      // tool's input and status.
      toolRow(9, 'tc-boundary', { acpEventKind: 'tool_call_update', acpToolStatus: 'completed' }),
      // Inside the window: always kept.
      toolRow(13, 'tc-new'),
      message(14, 'assistant', { content: '', acpEventKind: 'plan_update' }),
    ];
    expect(ids(metadataRowsForWindow(metadata, visible, true))).toEqual([9, 13, 14]);
  });
});
