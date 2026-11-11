import { beforeEach, describe, expect, it, vi } from 'vitest';

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
  loadReplyDraft,
  registerReplyComposer,
  replyDraftKey,
  replyDraftPresence,
  replyDraftWriter,
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

  it('amends the stored draft when no composer shows it', async () => {
    await saveDraft(replyDraftKey('s1'), 'Some thoughts first.');

    await addQuoteToReplyDraft('s1', quote);
    await addQuoteToReplyDraft('s1', { ...quote, reply: 'And this?' });

    const draft = await loadReplyDraft('s1');
    expect(draft.text).toBe('');
    expect(draft.quotes.map((q) => [q.before, q.reply])).toEqual([
      ['Some thoughts first.', 'Why?'],
      ['', 'And this?'],
    ]);
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
    const addQuote = vi.fn();
    const unregister = registerReplyComposer('s3', { addQuote });

    await addQuoteToReplyDraft('s3', quote);

    expect(addQuote).toHaveBeenCalledWith(quote);
    expect(store.has(replyDraftKey('s3'))).toBe(false);

    unregister();
    await addQuoteToReplyDraft('s3', quote);
    expect(addQuote).toHaveBeenCalledTimes(1);
    expect((await loadReplyDraft('s3')).quotes).toHaveLength(1);
  });

  it('keeps a newer composer registered when an older one unregisters', async () => {
    const older = vi.fn();
    const newer = vi.fn();
    const unregisterOlder = registerReplyComposer('s4', { addQuote: older });
    registerReplyComposer('s4', { addQuote: newer });
    unregisterOlder();

    await addQuoteToReplyDraft('s4', quote);

    expect(older).not.toHaveBeenCalled();
    expect(newer).toHaveBeenCalledWith(quote);
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

    // A quote amended into a stored draft.
    await addQuoteToReplyDraft('quoted', quote);
    expect(replyDraftPresence.has('quoted')).toBe(true);

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
