import { describe, expect, it } from 'vitest';
import {
  mergeFrameMessages,
  mergeSinceMessages,
  openAssistantRefetchIds,
  pollShouldStop,
  sessionPollRequest,
  unionSessionPollRequests,
} from './messagePoll';
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

  it('neither duplicates nor revives a cached row the fetched tail skips', () => {
    // Row 6 was cached but is absent from the tail (e.g. it no longer passes
    // the visible-message filter). A positional keep would re-keep row 7.
    const existing = [
      message(4, 'user', 'hi'),
      message(5, 'assistant', 'The'),
      message(6, 'tool_call', 'Read'),
      message(7, 'tool_result', 'ok'),
    ];
    const updated = [message(5, 'assistant', 'The end'), message(7, 'tool_result', 'ok')];
    const merged = mergeSinceMessages(existing, updated, 5);

    expect(merged!.appended).toBe(false);
    expect(merged!.messages.map((m) => m.id)).toEqual([4, 5, 7]);
    expect(merged!.messages[0]).toBe(existing[0]);
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

  it('keeps the inline body of a tail tool row that comes back settled and deferred', () => {
    const input = { file_path: '/tmp/a.txt', content: 'x'.repeat(3000) };
    const inFlight: SessionMessage = {
      ...message(3, 'tool_call', 'Write'),
      acpEventKind: 'tool_call',
      acpToolCallId: 'tc-3',
      acpToolStatus: 'in_progress',
      acpRawInput: input,
    };
    const settled: SessionMessage = {
      ...inFlight,
      acpToolStatus: 'completed',
      acpRawInput: {
        $deferred: { messageId: 3, field: 'input', bytes: JSON.stringify(input).length },
      },
      acpContent: [{ type: 'content', content: { type: 'text', text: 'ok' } }],
    };
    const existing = [message(2, 'assistant', 'The'), inFlight];
    const merged = mergeSinceMessages(existing, [settled, message(4, 'assistant', 'Done')], 3);

    expect(merged!.appended).toBe(true);
    expect(merged!.messages.map((m) => m.id)).toEqual([2, 3, 4]);
    expect(merged!.messages[1].acpToolStatus).toBe('completed');
    expect(merged!.messages[1].acpRawInput).toBe(input);
    expect(merged!.messages[1].acpContent).toBe(settled.acpContent);
  });
});

describe('sessionPollRequest', () => {
  function toolRow(id: number, status: string | undefined): SessionMessage {
    return {
      ...message(id, 'tool_call', ''),
      acpEventKind: 'tool_call',
      acpToolCallId: `tc-${id}`,
      acpToolStatus: status,
    };
  }

  it('polls an empty transcript from the start', () => {
    expect(sessionPollRequest([], [])).toEqual({
      sinceId: 0,
      refetchIds: [],
      metadataSinceId: 0,
      metadataRefetchIds: [],
    });
  });

  it('keys off the loaded tails and re-requests rows still being written', () => {
    const messages = [
      message(1, 'user', 'hi'),
      message(2, 'assistant', 'The'),
      message(3, 'tool_call', 'Read'),
    ];
    const metadata = [
      toolRow(4, 'completed'),
      toolRow(5, 'in_progress'),
      { ...message(6, 'assistant', ''), acpEventKind: 'plan_update' },
      toolRow(7, undefined),
    ];
    expect(sessionPollRequest(messages, metadata)).toEqual({
      sinceId: 3,
      refetchIds: [2],
      metadataSinceId: 7,
      metadataRefetchIds: [5, 7],
    });
  });
});

describe('mergeSinceMessages from an empty transcript', () => {
  it('takes every row a zero-cursor poll returns as appended', () => {
    const rows = [message(1, 'user', 'hi'), message(2, 'assistant', 'hello')];
    const merged = mergeSinceMessages([], rows, 0);

    expect(merged).not.toBeNull();
    expect(merged!.appended).toBe(true);
    expect(merged!.messages).toEqual(rows);
  });

  it('reports no change while the transcript is still empty', () => {
    expect(mergeSinceMessages([], [], 0)).toBeNull();
  });
});

/**
 * The cases above, as the session stream delivers them. A snapshot is read
 * from the pane's cursor, so it holds the rows the since-fetch returned; a
 * delta holds only the rows that were written. Either way the pane has to
 * end up with the transcript the poll would have given it.
 */
describe('mergeFrameMessages', () => {
  const settledWrite = (() => {
    const input = { file_path: '/tmp/a.txt', content: 'x'.repeat(3000) };
    const inFlight: SessionMessage = {
      ...message(3, 'tool_call', 'Write'),
      acpEventKind: 'tool_call',
      acpToolCallId: 'tc-3',
      acpToolStatus: 'in_progress',
      acpRawInput: input,
    };
    const settled: SessionMessage = {
      ...inFlight,
      acpToolStatus: 'completed',
      acpRawInput: {
        $deferred: { messageId: 3, field: 'input', bytes: JSON.stringify(input).length },
      },
      acpContent: [{ type: 'content', content: { type: 'text', text: 'ok' } }],
    };
    return { input, inFlight, settled };
  })();

  interface Case {
    name: string;
    existing: SessionMessage[];
    /** What a since-fetch from `lastId` returns, which is also a snapshot's rows. */
    fetched: SessionMessage[];
    lastId: number;
    /** The rows that were written since `existing` was read: a delta's rows. */
    written: SessionMessage[];
  }

  const cases: Case[] = [
    {
      name: 'an open assistant row behind the tail is extended',
      existing: [
        message(1, 'user', 'hi'),
        message(2, 'assistant', 'The'),
        message(3, 'tool_call', 'Read'),
        message(4, 'tool_result', 'ok'),
      ],
      fetched: [message(2, 'assistant', 'The search is back'), message(4, 'tool_result', 'ok')],
      lastId: 4,
      written: [message(2, 'assistant', 'The search is back')],
    },
    {
      name: 'nothing was written',
      existing: [
        message(2, 'assistant', 'The'),
        message(3, 'tool_call', 'Read'),
        message(4, 'tool_result', 'ok'),
      ],
      fetched: [message(2, 'assistant', 'The'), message(4, 'tool_result', 'ok')],
      lastId: 4,
      written: [],
    },
    {
      name: 'the tail streams',
      existing: [message(1, 'user', 'hi'), message(2, 'assistant', 'The')],
      fetched: [message(2, 'assistant', 'The end')],
      lastId: 2,
      written: [message(2, 'assistant', 'The end')],
    },
    {
      name: 'rows arrive past the tail while an earlier one is extended',
      existing: [message(2, 'assistant', 'The'), message(3, 'tool_call', 'Read')],
      fetched: [
        message(2, 'assistant', 'The search'),
        message(3, 'tool_call', 'Read'),
        message(4, 'tool_result', 'ok'),
      ],
      lastId: 3,
      written: [message(2, 'assistant', 'The search'), message(4, 'tool_result', 'ok')],
    },
    {
      name: 'a read from a cursor the pane has since moved past',
      existing: [
        message(4, 'user', 'hi'),
        message(5, 'assistant', 'The'),
        message(6, 'tool_call', 'Read'),
        message(7, 'tool_result', 'ok'),
      ],
      fetched: [
        message(5, 'assistant', 'The'),
        message(6, 'tool_call', 'Read'),
        message(7, 'tool_result', 'ok'),
        message(8, 'assistant', 'Done'),
      ],
      lastId: 5,
      written: [message(8, 'assistant', 'Done')],
    },
    {
      name: 'a read that ends before the rows the pane holds',
      existing: [
        message(4, 'user', 'hi'),
        message(5, 'assistant', 'The'),
        message(6, 'tool_call', 'Read'),
        message(7, 'tool_result', 'ok'),
      ],
      fetched: [message(5, 'assistant', 'The search'), message(6, 'tool_call', 'Read')],
      lastId: 5,
      written: [message(5, 'assistant', 'The search')],
    },
    {
      name: 'only a row’s metadata changed',
      existing: [message(2, 'assistant', 'The'), message(3, 'tool_call', 'Read')],
      fetched: [
        { ...message(2, 'assistant', 'The'), acpMessageId: 'msg-1' },
        message(3, 'tool_call', 'Read'),
      ],
      lastId: 3,
      written: [{ ...message(2, 'assistant', 'The'), acpMessageId: 'msg-1' }],
    },
    {
      name: 'a tool row settles with its body deferred and a row follows',
      existing: [message(2, 'assistant', 'The'), settledWrite.inFlight],
      fetched: [settledWrite.settled, message(4, 'assistant', 'Done')],
      lastId: 3,
      written: [settledWrite.settled, message(4, 'assistant', 'Done')],
    },
    {
      name: 'an empty transcript fills',
      existing: [],
      fetched: [message(1, 'user', 'hi'), message(2, 'assistant', 'hello')],
      lastId: 0,
      written: [message(1, 'user', 'hi'), message(2, 'assistant', 'hello')],
    },
  ];

  for (const frame of ['snapshot', 'delta'] as const) {
    it.each(cases)(`gives what the poll gives as a ${frame}: $name`, (c) => {
      const viaPoll = mergeSinceMessages(c.existing, c.fetched, c.lastId);
      const viaFrame = mergeFrameMessages(c.existing, frame === 'snapshot' ? c.fetched : c.written);

      expect(viaFrame === null).toBe(viaPoll === null);
      if (!viaPoll || !viaFrame) return;
      expect(viaFrame.messages).toEqual(viaPoll.messages);
      expect(viaFrame.appended).toBe(viaPoll.appended);
      // A row the update did not change keeps its identity, so its group
      // does not re-render.
      const changed = new Set(c.written.map((m) => m.id));
      for (const row of c.existing) {
        if (changed.has(row.id)) continue;
        expect(viaFrame.messages.find((m) => m.id === row.id)).toBe(row);
      }
    });
  }

  it('keeps the inline body of a tool row that settles', () => {
    const { input, inFlight, settled } = settledWrite;
    const merged = mergeFrameMessages([message(2, 'assistant', 'The'), inFlight], [settled]);

    expect(merged!.appended).toBe(false);
    expect(merged!.messages[1].acpToolStatus).toBe('completed');
    expect(merged!.messages[1].acpRawInput).toBe(input);
  });

  it('adds a row past the tail without touching the tail it follows', () => {
    // A since-fetch repeats the row at its cursor and splices from there. A
    // delta does not carry that row, and must not cost the pane it.
    const existing = [message(1, 'user', 'hi'), message(2, 'assistant', 'The')];
    const merged = mergeFrameMessages(existing, [message(3, 'tool_call', 'Read')]);

    expect(merged!.appended).toBe(true);
    expect(merged!.messages.map((m) => m.id)).toEqual([1, 2, 3]);
    expect(merged!.messages[1]).toBe(existing[1]);
  });

  it('applies the same delta twice with no change the second time', () => {
    const existing = [message(1, 'user', 'hi'), message(2, 'assistant', 'The')];
    const delta = [message(2, 'assistant', 'The end'), message(3, 'tool_call', 'Read')];
    const once = mergeFrameMessages(existing, delta);

    expect(mergeFrameMessages(once!.messages, delta)).toBeNull();
  });

  it('drops rows older than the window the pane holds', () => {
    // The pane holds rows 5 to 7 of a longer transcript. A snapshot read
    // from another pane's cursor, or a rewrite of an old row, reaches back
    // past them.
    const existing = [
      message(5, 'assistant', 'The'),
      message(6, 'tool_call', 'Read'),
      message(7, 'tool_result', 'ok'),
    ];
    const merged = mergeFrameMessages(existing, [
      message(2, 'assistant', 'Earlier'),
      message(4, 'tool_result', 'old'),
      message(7, 'tool_result', 'ok'),
      message(8, 'assistant', 'Done'),
    ]);

    expect(merged!.messages.map((m) => m.id)).toEqual([5, 6, 7, 8]);
    expect(mergeFrameMessages(existing, [message(2, 'assistant', 'Earlier')])).toBeNull();
  });

  it('puts a row the pane does not hold in id order', () => {
    const existing = [message(4, 'user', 'hi'), message(7, 'tool_result', 'ok')];
    const merged = mergeFrameMessages(existing, [message(6, 'tool_call', 'Read')]);

    expect(merged!.appended).toBe(false);
    expect(merged!.messages.map((m) => m.id)).toEqual([4, 6, 7]);
  });

  it('removes nothing: a frame says nothing about the rows it does not carry', () => {
    // The since-fetch drops a cached row its tail skips (see above). A
    // snapshot of the same rows leaves it.
    const existing = [
      message(4, 'user', 'hi'),
      message(5, 'assistant', 'The'),
      message(6, 'tool_call', 'Read'),
      message(7, 'tool_result', 'ok'),
    ];
    const merged = mergeFrameMessages(existing, [
      message(5, 'assistant', 'The end'),
      message(7, 'tool_result', 'ok'),
    ]);

    expect(merged!.messages.map((m) => m.id)).toEqual([4, 5, 6, 7]);
  });
});

describe('unionSessionPollRequests', () => {
  it('is the cursor itself for one pane', () => {
    const request = { sinceId: 7, refetchIds: [4], metadataSinceId: 9, metadataRefetchIds: [8] };
    expect(unionSessionPollRequests([request])).toEqual(request);
  });

  it('reads from the earliest cursor and every row either pane re-reads', () => {
    const behind = { sinceId: 40, refetchIds: [38], metadataSinceId: 35, metadataRefetchIds: [33] };
    const ahead = {
      sinceId: 90,
      refetchIds: [88, 38],
      metadataSinceId: 91,
      metadataRefetchIds: [89],
    };
    expect(unionSessionPollRequests([ahead, behind])).toEqual({
      sinceId: 40,
      refetchIds: [38, 88],
      metadataSinceId: 35,
      metadataRefetchIds: [33, 89],
    });
  });
});

describe('pollShouldStop', () => {
  it('keeps polling a running session', () => {
    expect(pollShouldStop({ status: 'running' }, false, 'running')).toBe(false);
  });

  it('stops once the fetched status is terminal and nothing fresher arrived', () => {
    expect(pollShouldStop({ status: 'completed' }, false, 'completed')).toBe(true);
  });

  it('stops on a disconnected session: it has no live connection to stream from', () => {
    expect(pollShouldStop({ status: 'disconnected' }, false, 'disconnected')).toBe(true);
  });

  it('defers to a status event that landed during the fetch', () => {
    expect(pollShouldStop({ status: 'completed' }, true, 'running')).toBe(false);
    expect(pollShouldStop({ status: 'completed' }, false, 'running')).toBe(false);
  });

  it('treats a missing session as terminal even when the pane last saw it running', () => {
    // A pooled pane reopened on a session deleted while it was parked: the
    // stale status says running, poll_session answers `session: null`.
    expect(pollShouldStop(null, false, 'running')).toBe(true);
    expect(pollShouldStop(null, true, 'running')).toBe(true);
  });
});
