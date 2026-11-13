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
  focusReplyDraftQuote,
  loadReplyDraft,
  registerReplyComposer,
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
});
