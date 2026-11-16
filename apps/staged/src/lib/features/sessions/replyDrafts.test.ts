import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';

const { store, changeListeners, establishers } = vi.hoisted(() => ({
  store: new Map<string, unknown>(),
  changeListeners: new Set<(key: string, value: unknown) => void>(),
  establishers: [] as Array<() => void>,
}));

vi.mock('../../shared/persistentStore', () => ({
  getStoreValue: vi.fn(async (key: string) => store.get(key)),
  setStoreValue: vi.fn(async (key: string, value: unknown) => {
    store.set(key, value);
  }),
  deleteStoreValue: vi.fn(async (key: string) => {
    store.delete(key);
  }),
  listStoreKeys: vi.fn(async () => [...store.keys()]),
  listenToStoreChanges: vi.fn(
    (callback: (key: string, value: unknown) => void, options?: { onEstablished?: () => void }) => {
      changeListeners.add(callback);
      if (options?.onEstablished) establishers.push(options.onEstablished);
      return () => changeListeners.delete(callback);
    }
  ),
}));

import { setStoreValue } from '../../shared/persistentStore';
import { saveDraft, DRAFT_TTL_MS } from '../../shared/draftCache';
import { serializeComposerDraft } from './composerDraft';
import {
  addQuoteToReplyDraft,
  editLiveQuoteReply,
  focusReplyDraftQuote,
  loadReplyDraft,
  registerReplyComposer,
  removeQuoteFromReplyDraft,
  replyDraftKey,
  replyDraftPresence,
  replyDraftWriter,
  setReplyDraftQuoteReply,
  startLiveQuoteReply,
  type ReplyComposerTarget,
} from './replyDrafts';

const quote = { quote: 'The cache is invalidated.', headings: ['Architecture'], reply: 'Why?' };

/** Let queued store writes run. */
async function settle() {
  await replyDraftWriter.flush();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe('addQuoteToReplyDraft', () => {
  beforeEach(() => {
    store.clear();
  });

  it('amends the stored draft when no composer shows it, under the id it resolves', async () => {
    await saveDraft(replyDraftKey('s1'), 'Some thoughts first.');

    const first = await addQuoteToReplyDraft('s1', quote);
    const second = await addQuoteToReplyDraft('s1', { ...quote, reply: 'And this?' }, 'given-id');

    const draft = await loadReplyDraft('s1');
    expect(draft.text).toBe('');
    expect(draft.quotes.map((q) => [q.id, q.before, q.reply])).toEqual([
      [first, 'Some thoughts first.', 'Why?'],
      ['given-id', '', 'And this?'],
    ]);
    expect(second).toBe('given-id');
    expect(store.get(replyDraftKey('s1'))).toMatchObject({ value: serializeComposerDraft(draft) });
  });

  it('runs behind a pending debounced save of the same draft', async () => {
    vi.useFakeTimers();
    try {
      replyDraftWriter.scheduleSave(replyDraftKey('s2'), 'typed just now');

      const added = addQuoteToReplyDraft('s2', quote);
      await vi.runAllTimersAsync();
      await added;

      const draft = await loadReplyDraft('s2');
      expect(draft.quotes[0]).toMatchObject({ before: 'typed just now', reply: 'Why?' });
    } finally {
      vi.useRealTimers();
    }
  });

  it('rejects when the stored draft cannot be amended, so the caller can say so', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    try {
      const failure = new Error('store unavailable');
      vi.mocked(setStoreValue).mockRejectedValueOnce(failure);

      await expect(addQuoteToReplyDraft('s5', quote)).rejects.toBe(failure);
      expect(store.has(replyDraftKey('s5'))).toBe(false);

      // The writer is still usable afterwards.
      await addQuoteToReplyDraft('s5', quote);
      expect((await loadReplyDraft('s5')).quotes).toHaveLength(1);
    } finally {
      consoleError.mockRestore();
    }
  });

  it('hands the quote to the open composer instead of the store', async () => {
    const composer = fakeComposer();
    const unregister = registerReplyComposer('s3', composer);

    const id = await addQuoteToReplyDraft('s3', quote);

    expect(composer.addQuote).toHaveBeenCalledWith(quote, id);
    expect(store.has(replyDraftKey('s3'))).toBe(false);

    unregister();
    await addQuoteToReplyDraft('s3', quote);
    expect(composer.addQuote).toHaveBeenCalledTimes(1);
    expect((await loadReplyDraft('s3')).quotes).toHaveLength(1);
  });

  it('keeps a newer composer registered when an older one unregisters', async () => {
    const older = fakeComposer();
    const newer = fakeComposer();
    const unregisterOlder = registerReplyComposer('s4', older);
    const unregisterNewer = registerReplyComposer('s4', newer);
    unregisterOlder();

    const id = await addQuoteToReplyDraft('s4', quote);

    expect(older.addQuote).not.toHaveBeenCalled();
    expect(newer.addQuote).toHaveBeenCalledWith(quote, id);
    expect(store.has(replyDraftKey('s4'))).toBe(false);
    unregisterNewer();
  });

  it('keeps the older composer when the newer one unregisters, as a second dialog on the session closes', async () => {
    const older = fakeComposer();
    const newer = fakeComposer();
    const unregisterOlder = registerReplyComposer('s6', older);
    const unregisterNewer = registerReplyComposer('s6', newer);
    unregisterNewer();

    const id = await addQuoteToReplyDraft('s6', quote);
    await setReplyDraftQuoteReply('s6', id, 'Because.');

    expect(newer.addQuote).not.toHaveBeenCalled();
    expect(older.addQuote).toHaveBeenCalledWith(quote, id);
    expect(older.setQuoteReply).toHaveBeenCalledWith(id, 'Because.');
    expect(focusReplyDraftQuote('s6', id)).toBe(true);
    expect(older.focusQuoteReply).toHaveBeenCalledWith(id);
    expect(newer.focusQuoteReply).not.toHaveBeenCalled();
    // Nothing went to the store under the open composer.
    expect(store.has(replyDraftKey('s6'))).toBe(false);

    unregisterOlder();
    expect(focusReplyDraftQuote('s6', id)).toBe(false);
    await addQuoteToReplyDraft('s6', quote);
    expect((await loadReplyDraft('s6')).quotes).toHaveLength(1);
  });

  it('gives every live composer the edit, and the caret to the newest', async () => {
    const older = fakeComposer();
    const newer = fakeComposer();
    const unregisterOlder = registerReplyComposer('s7', older);
    const unregisterNewer = registerReplyComposer('s7', newer);

    const id = await addQuoteToReplyDraft('s7', quote);
    await setReplyDraftQuoteReply('s7', id, 'Because.');
    expect(focusReplyDraftQuote('s7', id)).toBe(true);

    for (const composer of [older, newer]) {
      expect(composer.addQuote).toHaveBeenCalledWith(quote, id);
      expect(composer.setQuoteReply).toHaveBeenCalledWith(id, 'Because.');
    }
    expect(newer.focusQuoteReply).toHaveBeenCalledWith(id);
    expect(older.focusQuoteReply).not.toHaveBeenCalled();

    unregisterOlder();
    unregisterNewer();
  });
});

function fakeComposer(): { [K in keyof ReplyComposerTarget]: Mock<ReplyComposerTarget[K]> } {
  return {
    addQuote: vi.fn<ReplyComposerTarget['addQuote']>(),
    setQuoteReply: vi.fn<ReplyComposerTarget['setQuoteReply']>(),
    removeQuote: vi.fn<ReplyComposerTarget['removeQuote']>(),
    focusQuoteReply: vi.fn<ReplyComposerTarget['focusQuoteReply']>(),
  };
}

describe('setReplyDraftQuoteReply', () => {
  beforeEach(() => {
    store.clear();
  });

  it('rewrites the reply of a stored quote and leaves a removed one removed', async () => {
    const id = await addQuoteToReplyDraft('r1', { ...quote, reply: '' });
    await addQuoteToReplyDraft('r1', quote, 'other');

    await setReplyDraftQuoteReply('r1', id, 'Because of the cache.');
    await setReplyDraftQuoteReply('r1', 'gone', 'nowhere');

    const draft = await loadReplyDraft('r1');
    expect(draft.quotes.map((q) => [q.id, q.reply])).toEqual([
      [id, 'Because of the cache.'],
      ['other', 'Why?'],
    ]);
  });

  it('goes to the open composer, as does focusing the quote', async () => {
    const composer = fakeComposer();
    const unregister = registerReplyComposer('r2', composer);

    await setReplyDraftQuoteReply('r2', 'q', 'typed');
    expect(composer.setQuoteReply).toHaveBeenCalledWith('q', 'typed');
    expect(store.has(replyDraftKey('r2'))).toBe(false);

    expect(focusReplyDraftQuote('r2', 'q')).toBe(true);
    expect(composer.focusQuoteReply).toHaveBeenCalledWith('q');

    unregister();
    expect(focusReplyDraftQuote('r2', 'q')).toBe(false);
  });
});

describe('removeQuoteFromReplyDraft', () => {
  beforeEach(() => {
    store.clear();
  });

  it('takes the block out of the stored draft and keeps the text around it', async () => {
    await saveDraft(replyDraftKey('x1'), 'Before.');
    const first = await addQuoteToReplyDraft('x1', quote);
    const second = await addQuoteToReplyDraft('x1', { ...quote, reply: 'Kept.' });

    await removeQuoteFromReplyDraft('x1', first);
    await removeQuoteFromReplyDraft('x1', 'never-there');

    const draft = await loadReplyDraft('x1');
    expect(draft.quotes.map((q) => [q.id, q.before, q.reply])).toEqual([
      [second, 'Before.', 'Kept.'],
    ]);
  });

  it('goes to every open composer rather than the store under it', async () => {
    const older = fakeComposer();
    const newer = fakeComposer();
    const unregisterOlder = registerReplyComposer('x2', older);
    const unregisterNewer = registerReplyComposer('x2', newer);

    await removeQuoteFromReplyDraft('x2', 'q');

    expect(older.removeQuote).toHaveBeenCalledWith('q', undefined);
    expect(newer.removeQuote).toHaveBeenCalledWith('q', undefined);
    expect(store.has(replyDraftKey('x2'))).toBe(false);

    unregisterOlder();
    unregisterNewer();
  });

  it('with a condition, removes only a block it holds of as the stored draft has it', async () => {
    const answered = await addQuoteToReplyDraft('x3', quote);
    const unanswered = await addQuoteToReplyDraft('x3', { ...quote, reply: '' });
    const empty = (q: { reply: string }) => q.reply === '';

    await removeQuoteFromReplyDraft('x3', answered, empty);
    await removeQuoteFromReplyDraft('x3', unanswered, empty);

    expect((await loadReplyDraft('x3')).quotes.map((q) => q.id)).toEqual([answered]);
  });

  it('hands the condition to every open composer, which has the block to ask it of', async () => {
    const composer = fakeComposer();
    const unregister = registerReplyComposer('x4', composer);
    const empty = (q: { reply: string }) => q.reply === '';

    await removeQuoteFromReplyDraft('x4', 'q', empty);

    expect(composer.removeQuote).toHaveBeenCalledWith('q', empty);
    unregister();
  });
});

describe('editLiveQuoteReply', () => {
  beforeEach(() => {
    store.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('updates the block it was opened on and never adds another', async () => {
    const id = await addQuoteToReplyDraft('e1', quote);
    await addQuoteToReplyDraft('e1', { ...quote, reply: 'Other.' }, 'other');

    const live = editLiveQuoteReply('e1', id, 'Why?', 100);
    expect(live.id).toBe(id);
    live.setReply('Why so');
    live.setReply('Why so often?');
    await vi.advanceTimersByTimeAsync(99);
    expect((await loadReplyDraft('e1')).quotes[0].reply).toBe('Why?');
    await vi.advanceTimersByTimeAsync(1);
    await vi.runAllTimersAsync();
    expect((await loadReplyDraft('e1')).quotes[0].reply).toBe('Why so often?');

    live.setReply('Because.');
    await live.finish();

    const draft = await loadReplyDraft('e1');
    expect(draft.quotes.map((q) => [q.id, q.reply])).toEqual([
      [id, 'Because.'],
      ['other', 'Other.'],
    ]);
  });

  it('writes nothing when the field closes on an unchanged reply', async () => {
    const id = await addQuoteToReplyDraft('e2', quote);
    vi.mocked(setStoreValue).mockClear();

    const live = editLiveQuoteReply('e2', id, 'Why?', 100);
    live.setReply('Why?');
    await live.finish();

    expect(setStoreValue).not.toHaveBeenCalled();
  });

  it('leaves a block removed in the composer meanwhile removed', async () => {
    const id = await addQuoteToReplyDraft('e3', quote);
    const live = editLiveQuoteReply('e3', id, 'Why?', 100);
    await removeQuoteFromReplyDraft('e3', id);

    live.setReply('Typed after it went');
    await live.finish();

    expect((await loadReplyDraft('e3')).quotes).toEqual([]);
  });

  it('discards its block, with what was typed and not yet written', async () => {
    const id = await addQuoteToReplyDraft('e4', quote);
    await addQuoteToReplyDraft('e4', { ...quote, reply: 'Other.' }, 'other');
    const live = editLiveQuoteReply('e4', id, 'Why?', 100);

    // Cleared, and closed before the pause that would have written it: the
    // draft still holds the reply the field opened on.
    live.setReply('');
    await live.discard();
    // Nothing typed, or finished, after the removal brings it back; nor
    // does the write that was pending.
    live.setReply('Too late');
    await live.finish();
    await vi.runAllTimersAsync();

    expect((await loadReplyDraft('e4')).quotes.map((q) => q.id)).toEqual(['other']);
  });

  it('discards a block that holds a reply it wrote itself before the field was cleared', async () => {
    const id = await addQuoteToReplyDraft('e6', quote);
    const live = editLiveQuoteReply('e6', id, 'Why?', 100);
    live.setReply('Why so often?');
    await vi.advanceTimersByTimeAsync(100);
    await vi.runAllTimersAsync();
    expect((await loadReplyDraft('e6')).quotes[0].reply).toBe('Why so often?');

    live.setReply('');
    await live.discard();

    expect((await loadReplyDraft('e6')).quotes).toEqual([]);
  });

  it('discards a block whose cleared reply was already written', async () => {
    const id = await addQuoteToReplyDraft('e7', quote);
    const live = editLiveQuoteReply('e7', id, 'Why?', 100);
    live.setReply('  ');
    await vi.advanceTimersByTimeAsync(100);
    await vi.runAllTimersAsync();
    expect((await loadReplyDraft('e7')).quotes[0].reply).toBe('  ');

    await live.discard();

    expect((await loadReplyDraft('e7')).quotes).toEqual([]);
  });

  it('does not discard a block given a reply elsewhere while its field was open', async () => {
    const id = await addQuoteToReplyDraft('e8', { ...quote, reply: '' });
    const live = editLiveQuoteReply('e8', id, '', 100);
    // Typed into the block in a composer, or by another client, meanwhile.
    await setReplyDraftQuoteReply('e8', id, 'Answered in the chat.');

    await live.discard();

    expect((await loadReplyDraft('e8')).quotes.map((q) => [q.id, q.reply])).toEqual([
      [id, 'Answered in the chat.'],
    ]);
  });

  it('does not discard a block whose reply was changed elsewhere from the one it opened on', async () => {
    const id = await addQuoteToReplyDraft('e9', quote);
    const live = editLiveQuoteReply('e9', id, 'Why?', 100);
    live.setReply('');
    await setReplyDraftQuoteReply('e9', id, 'Why? I see now: the readers.');

    await live.discard();

    expect((await loadReplyDraft('e9')).quotes[0].reply).toBe('Why? I see now: the readers.');
  });

  it('discards behind the writes of a field that was on the block just before', async () => {
    const id = await addQuoteToReplyDraft('e10', quote);
    const first = editLiveQuoteReply('e10', id, 'Why?', 100);
    first.setReply('Why so');
    // The store is slow to take the first field's write.
    let release!: () => void;
    const slow = new Promise<void>((resolve) => (release = resolve));
    vi.mocked(setStoreValue).mockImplementationOnce(async (key: string, value: unknown) => {
      await slow;
      store.set(key, value);
    });
    try {
      await vi.advanceTimersByTimeAsync(100);
      // More is typed and the field closes: that write waits on the slow one.
      first.setReply('Why so often?');
      const finished = first.finish();

      // Reopened at once on what was typed, cleared and closed again.
      const second = editLiveQuoteReply('e10', id, 'Why so often?', 100);
      second.setReply('');
      const discarded = second.discard();
      release();
      await finished;
      await discarded;
      await vi.runAllTimersAsync();
    } finally {
      // Never left holding the shared writer's queue, whatever failed above.
      release();
    }

    // Gone, and the first field's last write has not put it back.
    expect((await loadReplyDraft('e10')).quotes).toEqual([]);
  });

  it('retries from finish when its write failed', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    try {
      const id = await addQuoteToReplyDraft('e5', quote);
      const live = editLiveQuoteReply('e5', id, 'Why?', 100);
      live.setReply('Edited');
      vi.mocked(setStoreValue).mockRejectedValueOnce(new Error('store unavailable'));
      await vi.advanceTimersByTimeAsync(100);
      await vi.runAllTimersAsync();
      expect((await loadReplyDraft('e5')).quotes[0].reply).toBe('Why?');

      await live.finish();
      expect((await loadReplyDraft('e5')).quotes[0].reply).toBe('Edited');
    } finally {
      consoleError.mockRestore();
    }
  });
});

describe('startLiveQuoteReply', () => {
  beforeEach(() => {
    store.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('adds the block at once and writes the reply after each pause in typing', async () => {
    const live = startLiveQuoteReply('l1', { ...quote, reply: '' }, 100);
    await vi.runAllTimersAsync();
    expect((await loadReplyDraft('l1')).quotes).toMatchObject([{ reply: '' }]);

    live.setReply('Wh');
    live.setReply('Why');
    await vi.advanceTimersByTimeAsync(99);
    expect((await loadReplyDraft('l1')).quotes[0].reply).toBe('');
    await vi.advanceTimersByTimeAsync(1);
    await vi.runAllTimersAsync();
    expect((await loadReplyDraft('l1')).quotes[0].reply).toBe('Why');

    live.setReply('Why not?');
    await live.finish();
    const draft = await loadReplyDraft('l1');
    expect(draft.quotes).toHaveLength(1);
    expect(draft.quotes[0].reply).toBe('Why not?');
    // The id it reports is the block's, known before the add has landed.
    expect(draft.quotes[0].id).toBe(live.id);
  });

  it('discards the block it added, even before the add has landed', async () => {
    const live = startLiveQuoteReply('l5', { ...quote, reply: '' }, 100);
    live.setReply('Second thoughts');
    live.setReply('');
    await live.discard();
    await vi.runAllTimersAsync();

    expect((await loadReplyDraft('l5')).quotes).toEqual([]);
    expect(replyDraftPresence.quotes('l5')).toEqual([]);
  });

  it('does not discard the block it added once a composer holds a reply to it', async () => {
    const live = startLiveQuoteReply('l6', { ...quote, reply: '' }, 100);
    await vi.runAllTimersAsync();
    // The chat opens with the block, and the reply is typed there.
    const composer = fakeComposer();
    const unregister = registerReplyComposer('l6', composer);

    await live.discard();

    // The composer is asked to remove the block only while its reply is
    // still the empty one the field left it with.
    expect(composer.removeQuote).toHaveBeenCalledTimes(1);
    const [id, when] = composer.removeQuote.mock.calls[0];
    expect(id).toBe(live.id);
    const block = { id: live.id, before: '', quote: quote.quote, headings: quote.headings };
    expect(when!({ ...block, reply: '' })).toBe(true);
    expect(when!({ ...block, reply: ' \n' })).toBe(true);
    expect(when!({ ...block, reply: 'Typed in the chat.' })).toBe(false);
    unregister();
  });

  it('keeps one block when the chat opens mid-reply, finishing into its composer', async () => {
    const live = startLiveQuoteReply('l2', { ...quote, reply: '' }, 100);
    await vi.runAllTimersAsync();
    const stored = await loadReplyDraft('l2');
    expect(stored.quotes).toHaveLength(1);

    // The composer opens: it loads the stored block, then takes the edits.
    const composer = fakeComposer();
    registerReplyComposer('l2', composer);
    live.setReply('Typed after the chat opened');
    await live.finish();

    expect(composer.addQuote).not.toHaveBeenCalled();
    expect(composer.setQuoteReply).toHaveBeenCalledWith(
      stored.quotes[0].id,
      'Typed after the chat opened'
    );
    expect((await loadReplyDraft('l2')).quotes).toHaveLength(1);
  });

  it('retries from finish when the first write failed, so the typed reply still lands', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    try {
      vi.mocked(setStoreValue).mockRejectedValueOnce(new Error('store unavailable'));
      const live = startLiveQuoteReply('l3', { ...quote, reply: '' }, 100);
      await vi.runAllTimersAsync();
      expect(store.has(replyDraftKey('l3'))).toBe(false);

      live.setReply('Still typed');
      await live.finish();

      const draft = await loadReplyDraft('l3');
      expect(draft.quotes).toHaveLength(1);
      expect(draft.quotes[0]).toMatchObject({ quote: quote.quote, reply: 'Still typed' });
    } finally {
      consoleError.mockRestore();
    }
  });

  it('rejects from finish when nothing could be written', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    try {
      const failure = new Error('store unavailable');
      vi.mocked(setStoreValue).mockRejectedValue(failure);
      const live = startLiveQuoteReply('l4', quote, 100);
      await vi.runAllTimersAsync();

      await expect(live.finish()).rejects.toBe(failure);
    } finally {
      vi.mocked(setStoreValue).mockReset();
      vi.mocked(setStoreValue).mockImplementation(async (key: string, value: unknown) => {
        store.set(key, value);
      });
      consoleError.mockRestore();
    }
  });
});

describe('replyDraftPresence', () => {
  beforeEach(async () => {
    store.clear();
  });

  it('hydrates from the store, then follows local writes and other clients', async () => {
    const now = Date.now();
    store.set(replyDraftKey('stored'), { value: 'hi', updatedAt: now, expiresAt: now + 1000 });
    store.set(replyDraftKey('expired'), { value: 'old', updatedAt: 0, expiresAt: now - 1 });
    store.set(replyDraftKey('blank'), { value: '  ', updatedAt: now, expiresAt: now + 1000 });
    store.set('draft:project-session:p1', { value: 'x', updatedAt: now, expiresAt: now + 1000 });

    // Read at start, before the event stream is live.
    replyDraftPresence.start();
    await settle();

    expect(replyDraftPresence.has('stored')).toBe(true);
    expect(replyDraftPresence.summary('stored')).toEqual({ quotes: 0, hasText: true });
    expect(replyDraftPresence.has('expired')).toBe(false);
    expect(replyDraftPresence.has('blank')).toBe(false);
    expect(replyDraftPresence.has('p1')).toBe(false);
    expect(replyDraftPresence.has(null)).toBe(false);

    // Read again once it is, for whatever changed before then.
    store.set(replyDraftKey('missed'), { value: 'hi', updatedAt: now, expiresAt: now + 1000 });
    store.delete(replyDraftKey('stored'));
    for (const establish of establishers) establish();
    await settle();

    expect(replyDraftPresence.has('missed')).toBe(true);
    expect(replyDraftPresence.has('stored')).toBe(false);

    // This client's writes.
    await saveDraft(replyDraftKey('typed'), 'a reply');
    expect(replyDraftPresence.has('typed')).toBe(true);
    await replyDraftWriter.clearNow(replyDraftKey('typed'));
    expect(replyDraftPresence.has('typed')).toBe(false);

    // A quote amended into a stored draft, counted for the note's bottom bar.
    await addQuoteToReplyDraft('quoted', { ...quote, reply: '' });
    expect(replyDraftPresence.has('quoted')).toBe(true);
    expect(replyDraftPresence.summary('quoted')).toEqual({ quotes: 1, hasText: false });
    await addQuoteToReplyDraft('quoted', quote);
    expect(replyDraftPresence.summary('quoted')).toEqual({ quotes: 2, hasText: true });
    expect(replyDraftPresence.summary('typed')).toBeNull();
    expect(replyDraftPresence.summary('stored')).toBeNull();

    // Another client's write, then its delete.
    for (const listener of changeListeners) {
      listener(replyDraftKey('remote'), {
        value: 'elsewhere',
        updatedAt: now,
        expiresAt: now + DRAFT_TTL_MS,
      });
    }
    expect(replyDraftPresence.has('remote')).toBe(true);
    for (const listener of changeListeners) listener(replyDraftKey('remote'), undefined);
    expect(replyDraftPresence.has('remote')).toBe(false);
  });

  it("holds each draft's quote blocks, for the note to mark their passages", async () => {
    replyDraftPresence.start();
    await settle();
    expect(replyDraftPresence.quotes('marks')).toEqual([]);
    expect(replyDraftPresence.quotes(null)).toEqual([]);

    const anchor = { prefix: 'before', suffix: 'after', offset: 12 };
    const first = await addQuoteToReplyDraft('marks', { ...quote, anchor });
    const second = await addQuoteToReplyDraft('marks', { ...quote, reply: '' });
    const held = replyDraftPresence.quotes('marks');
    expect(held.map((q) => [q.id, q.reply, q.anchor])).toEqual([
      [first, 'Why?', anchor],
      [second, '', undefined],
    ]);

    // The same array while no block changes, so a reader isn't re-run...
    await replyDraftWriter.saveNow(replyDraftKey('marks'), {
      value: 'whatever',
      structured: { version: 1, quotes: [...held], text: 'Free text typed below.' },
    });
    expect(replyDraftPresence.quotes('marks')).toBe(held);
    // ...and a new one when a reply is edited or a block goes.
    await setReplyDraftQuoteReply('marks', second, 'Edited.');
    expect(replyDraftPresence.quotes('marks')).not.toBe(held);
    expect(replyDraftPresence.quotes('marks')[1].reply).toBe('Edited.');
    await removeQuoteFromReplyDraft('marks', first);
    expect(replyDraftPresence.quotes('marks').map((q) => q.id)).toEqual([second]);

    // Another client's write, structure and all.
    const now = Date.now();
    for (const listener of changeListeners) {
      listener(replyDraftKey('marks'), {
        value: 'elsewhere',
        updatedAt: now,
        expiresAt: now + DRAFT_TTL_MS,
        structured: {
          version: 1,
          quotes: [{ id: 'remote', before: '', quote: 'x', headings: [], reply: '' }],
          text: '',
        },
      });
    }
    expect(replyDraftPresence.quotes('marks').map((q) => q.id)).toEqual(['remote']);

    // Sent: the draft is cleared and its quotes go with it.
    await replyDraftWriter.clearNow(replyDraftKey('marks'));
    expect(replyDraftPresence.quotes('marks')).toEqual([]);
    expect(replyDraftPresence.has('marks')).toBe(false);
  });

  it("takes an open composer's draft ahead of its debounced write", async () => {
    replyDraftPresence.start();
    await settle();
    const block = { id: 'live', before: '', quote: 'x', headings: [], reply: '' };

    replyDraftPresence.preview('shown', { quotes: [block], text: '' });
    expect(replyDraftPresence.quotes('shown')).toEqual([block]);
    expect(replyDraftPresence.summary('shown')).toEqual({ quotes: 1, hasText: false });
    expect(store.has(replyDraftKey('shown'))).toBe(false);

    // The block removed in the composer, leaving plain text; then nothing.
    replyDraftPresence.preview('shown', { quotes: [], text: 'Just text now.' });
    expect(replyDraftPresence.quotes('shown')).toEqual([]);
    expect(replyDraftPresence.has('shown')).toBe(true);
    replyDraftPresence.preview('shown', { quotes: [], text: '' });
    expect(replyDraftPresence.has('shown')).toBe(false);
  });

  it('keeps the same summary and quotes while a draft changes nothing they say', async () => {
    replyDraftPresence.start();
    await settle();
    const block = { id: 'typed', before: '', quote: 'x', headings: ['H'], reply: 'Why?' };

    replyDraftPresence.preview('typing', { quotes: [block], text: 'So' });
    const summary = replyDraftPresence.summary('typing');
    const quotes = replyDraftPresence.quotes('typing');
    // Copies, not the composer's own blocks, which it goes on editing.
    expect(quotes).toEqual([block]);
    expect(quotes[0]).not.toBe(block);
    expect(quotes[0].headings).not.toBe(block.headings);

    // Each keystroke in the main input reports the draft again: a reader of
    // the summary or the quotes has nothing new to read, and is given the
    // same objects, so it is not re-run.
    replyDraftPresence.preview('typing', { quotes: [{ ...block }], text: 'So far' });
    expect(replyDraftPresence.summary('typing')).toBe(summary);
    expect(replyDraftPresence.quotes('typing')).toBe(quotes);

    // What they say changing replaces them.
    replyDraftPresence.preview('typing', {
      quotes: [{ ...block, reply: '' }],
      text: '',
    });
    expect(replyDraftPresence.summary('typing')).toEqual({ quotes: 1, hasText: false });
    expect(replyDraftPresence.summary('typing')).not.toBe(summary);
    expect(replyDraftPresence.quotes('typing')).not.toBe(quotes);
  });
});
