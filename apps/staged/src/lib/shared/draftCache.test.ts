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

import {
  createDraftWriteScheduler,
  DRAFT_TTL_MS,
  DRAFT_WRITE_DEBOUNCE_MS,
  loadDraft,
  saveDraft,
} from './draftCache';

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

describe('draft write scheduler', () => {
  beforeEach(() => {
    store.clear();
    vi.useFakeTimers();
    return () => vi.useRealTimers();
  });

  async function drainWrites() {
    await vi.advanceTimersByTimeAsync(DRAFT_WRITE_DEBOUNCE_MS);
  }

  it('debounces writes for the same key, keeping only the latest value', async () => {
    const scheduler = createDraftWriteScheduler();

    scheduler.scheduleSave('draft:a', 'hel');
    scheduler.scheduleSave('draft:a', 'hello');
    await drainWrites();

    expect(store.get('draft:a')).toMatchObject({ value: 'hello' });
  });

  it('flushes a pending write when scheduling for a different key', async () => {
    const scheduler = createDraftWriteScheduler();

    scheduler.scheduleSave('draft:a', 'from session a');
    scheduler.scheduleSave('draft:b', 'from session b');
    await drainWrites();

    expect(store.get('draft:a')).toMatchObject({ value: 'from session a' });
    expect(store.get('draft:b')).toMatchObject({ value: 'from session b' });
  });

  it('flushes a pending write when saveNow/clearNow target a different key', async () => {
    const scheduler = createDraftWriteScheduler();

    scheduler.scheduleSave('draft:a', 'pending a');
    await scheduler.saveNow('draft:b', 'immediate b');
    scheduler.scheduleSave('draft:c', 'pending c');
    await scheduler.clearNow('draft:b');
    await drainWrites();

    expect(store.get('draft:a')).toMatchObject({ value: 'pending a' });
    expect(store.get('draft:c')).toMatchObject({ value: 'pending c' });
    expect(store.has('draft:b')).toBe(false);
  });

  it('supersedes a pending write when saveNow/clearNow target the same key', async () => {
    const scheduler = createDraftWriteScheduler();

    scheduler.scheduleSave('draft:a', 'stale');
    await scheduler.saveNow('draft:a', 'fresh');
    scheduler.scheduleSave('draft:b', 'stale');
    await scheduler.clearNow('draft:b');
    await drainWrites();

    expect(store.get('draft:a')).toMatchObject({ value: 'fresh' });
    expect(store.has('draft:b')).toBe(false);
  });

  it('flush persists a pending write without waiting for the debounce', async () => {
    const scheduler = createDraftWriteScheduler();

    scheduler.scheduleSave('draft:a', 'typed right before close');
    await scheduler.flush();

    expect(store.get('draft:a')).toMatchObject({ value: 'typed right before close' });

    await drainWrites();
    expect(store.get('draft:a')).toMatchObject({ value: 'typed right before close' });
  });

  it('flush with nothing pending resolves without writing', async () => {
    const scheduler = createDraftWriteScheduler();

    await scheduler.flush();

    expect(store.size).toBe(0);
  });
});
