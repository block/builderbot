import { beforeEach, describe, expect, it, vi } from 'vitest';

const store = vi.hoisted(() => new Map<string, unknown>());

vi.mock('./persistentStore', () => ({
  getStoreValue: vi.fn(async (key: string) => store.get(key)),
  setStoreValue: vi.fn(async (key: string, value: unknown) => {
    store.set(key, value);
  }),
  deleteStoreValue: vi.fn(async (key: string) => {
    store.delete(key);
  }),
}));

import { DRAFT_TTL_MS, loadDraft, saveDraft } from './draftCache';

describe('draft cache', () => {
  beforeEach(() => {
    store.clear();
  });

  it('restores a saved draft before expiry', async () => {
    const now = 1_000;

    await saveDraft('draft:one', 'hello', now);

    await expect(loadDraft('draft:one', now + DRAFT_TTL_MS - 1)).resolves.toBe('hello');
  });

  it('deletes and ignores expired drafts', async () => {
    const now = 2_000;

    await saveDraft('draft:expired', 'old', now);

    await expect(loadDraft('draft:expired', now + DRAFT_TTL_MS)).resolves.toBeNull();
    expect(store.has('draft:expired')).toBe(false);
  });

  it('deletes entries for empty values', async () => {
    await saveDraft('draft:empty', 'existing', 3_000);

    await saveDraft('draft:empty', '', 4_000);

    expect(store.has('draft:empty')).toBe(false);
  });

  it('keeps separate keys isolated', async () => {
    await saveDraft('draft:first', 'first', 5_000);
    await saveDraft('draft:second', 'second', 5_000);

    await expect(loadDraft('draft:first', 5_001)).resolves.toBe('first');
    await expect(loadDraft('draft:second', 5_001)).resolves.toBe('second');
  });
});
