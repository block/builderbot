import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// ── Mock plumbing ──

let getStoreValue: ReturnType<typeof vi.fn>;
let setStoreValue: ReturnType<typeof vi.fn>;
let savedWidth: unknown;

async function importDialogWidth() {
  return await import('./dialogWidth.svelte');
}

beforeEach(() => {
  vi.resetModules();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state as identity (navigation.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);
  // The module reads `window.innerWidth` for its maximum; tests run in node.
  vi.stubGlobal('window', { innerWidth: 1200 });

  savedWidth = undefined;
  getStoreValue = vi.fn().mockImplementation(() => Promise.resolve(savedWidth));
  setStoreValue = vi.fn().mockResolvedValue(undefined);

  vi.doMock('../../../shared/persistentStore', () => ({ getStoreValue, setStoreValue }));
});

afterEach(() => {
  vi.doUnmock('../../../shared/persistentStore');
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('createDialogWidth', () => {
  it('starts at the minimum so an undragged dialog keeps its old width', async () => {
    const { createDialogWidth } = await importDialogWidth();

    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });

    expect(width.width).toBe(700);
    expect(width.hydrated).toBe(false);
    expect(width.style).toBe('width:700px;max-width:calc(100vw - 64px);');
  });

  it('clamps below the minimum up to the minimum', async () => {
    const { createDialogWidth } = await importDialogWidth();

    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    width.set(200);

    expect(width.width).toBe(700);
  });

  it('keeps the preferred width independent of the viewport cap', async () => {
    const { createDialogWidth } = await importDialogWidth();

    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    width.set(5000);

    expect(width.width).toBe(5000);
    expect(width.style).toBe('width:5000px;max-width:calc(100vw - 64px);');
  });

  it('keeps the minimum when the window is narrower than it', async () => {
    vi.stubGlobal('window', { innerWidth: 500 });
    const { createDialogWidth } = await importDialogWidth();

    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    width.set(500);

    expect(width.width).toBe(700);
  });

  it('restores an oversized preference after starting in a small window and growing', async () => {
    savedWidth = 1400;
    const { createDialogWidth } = await importDialogWidth();

    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    await width.ensureHydrated();

    expect(getStoreValue).toHaveBeenCalledWith('test-width');
    expect(width.width).toBe(1400);
    expect(width.hydrated).toBe(true);
    vi.stubGlobal('window', { innerWidth: 1800 });
    expect(width.style).toBe('width:1400px;max-width:calc(100vw - 64px);');
    expect(setStoreValue).not.toHaveBeenCalled();
  });

  it.each([NaN, Infinity, -Infinity, '900', null])(
    'ignores invalid saved width %s',
    async (saved) => {
      savedWidth = saved;
      const { createDialogWidth } = await importDialogWidth();
      const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
      await width.ensureHydrated();
      expect(width.width).toBe(700);
      expect(width.hydrated).toBe(true);
    }
  );

  it.each([
    [200, 700],
    [900.7, 901],
  ])('normalizes saved width %s to %s', async (saved, expected) => {
    savedWidth = saved;
    const { createDialogWidth } = await importDialogWidth();
    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    await width.ensureHydrated();
    expect(width.width).toBe(expected);
  });

  it('settles a failed hydration once at the current width', async () => {
    const error = new Error('read failed');
    getStoreValue.mockRejectedValue(error);
    const log = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { createDialogWidth } = await importDialogWidth();
    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    await expect(width.ensureHydrated()).resolves.toBeUndefined();
    await width.ensureHydrated();
    expect(width.hydrated).toBe(true);
    expect(width.width).toBe(700);
    expect(getStoreValue).toHaveBeenCalledTimes(1);
    expect(log).toHaveBeenCalledWith('[DialogWidth] Failed to read test-width:', error);
  });

  it.each([true, false])(
    'ignores a delayed read after interaction (persist=%s)',
    async (persist) => {
      let resolve!: (width: number) => void;
      getStoreValue.mockReturnValue(new Promise<number>((done) => (resolve = done)));
      const { createDialogWidth } = await importDialogWidth();
      const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
      const hydration = width.ensureHydrated();
      width.set(1000, persist);
      resolve(800);
      await hydration;
      expect(width.width).toBe(1000);
      width.clearPreview();
      expect(width.width).toBe(persist ? 1000 : 700);
    }
  );

  it('does not apply a stale preference when hydration starts after a resize', async () => {
    savedWidth = 800;
    const { createDialogWidth } = await importDialogWidth();
    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    width.set(1000);
    await width.ensureHydrated();
    expect(width.width).toBe(1000);
  });

  it('logs rejected writes while keeping the new preference in memory', async () => {
    const error = new Error('write failed');
    setStoreValue.mockRejectedValue(error);
    const log = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { createDialogWidth } = await importDialogWidth();
    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    width.set(900);
    await Promise.resolve();
    expect(width.width).toBe(900);
    expect(log).toHaveBeenCalledWith('[DialogWidth] Failed to write test-width:', error);
  });

  it('discards a preview without losing an oversized preference or writing', async () => {
    savedWidth = 1400;
    const { createDialogWidth } = await importDialogWidth();
    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    await width.ensureHydrated();
    width.set(1000, false);
    expect(width.style).toContain('width:1000px;');
    width.clearPreview();
    expect(width.width).toBe(1400);
    expect(setStoreValue).not.toHaveBeenCalled();
  });

  it('leaves the default in place when nothing is saved, and hydrates once', async () => {
    const { createDialogWidth } = await importDialogWidth();

    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    await Promise.all([width.ensureHydrated(), width.ensureHydrated()]);
    await width.ensureHydrated();

    expect(width.width).toBe(700);
    expect(width.hydrated).toBe(true);
    expect(getStoreValue).toHaveBeenCalledTimes(1);
  });

  it('persists only when asked, so a drag writes once on release', async () => {
    const { createDialogWidth } = await importDialogWidth();

    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    width.set(800, false);
    width.set(900, false);

    expect(width.width).toBe(900);
    expect(setStoreValue).not.toHaveBeenCalled();

    width.set(900);

    expect(setStoreValue).toHaveBeenCalledExactlyOnceWith('test-width', 900);
  });

  it('persists the clamped width, not the requested one', async () => {
    const { createDialogWidth } = await importDialogWidth();

    createDialogWidth({ key: 'test-width', minWidth: 700 }).set(100);

    expect(setStoreValue).toHaveBeenCalledWith('test-width', 700);
  });

  it('resets to the default and persists that', async () => {
    const { createDialogWidth } = await importDialogWidth();

    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    width.set(900);
    width.reset();

    expect(width.width).toBe(700);
    expect(setStoreValue).toHaveBeenLastCalledWith('test-width', 700);
  });

  it('shares one instance per key so remounts keep the width', async () => {
    const { createDialogWidth } = await importDialogWidth();

    const first = createDialogWidth({ key: 'test-width', minWidth: 700 });
    first.set(900);
    const second = createDialogWidth({ key: 'test-width', minWidth: 700 });

    expect(second).toBe(first);
    expect(second.width).toBe(900);
  });

  it('keeps separate keys independent', async () => {
    const { createDialogWidth } = await importDialogWidth();

    const note = createDialogWidth({ key: 'note-width', minWidth: 700 });
    const newSession = createDialogWidth({ key: 'new-session-width', minWidth: 580 });
    note.set(900);

    expect(newSession.width).toBe(580);
  });

  it('ignores a non-finite width', async () => {
    const { createDialogWidth } = await importDialogWidth();

    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    width.set(900, false);
    width.set(Number.NaN);

    expect(width.width).toBe(900);
    expect(setStoreValue).not.toHaveBeenCalled();
  });
});

describe('hydrateDialogWidths', () => {
  it('hydrates every known dialog key', async () => {
    savedWidth = 900;
    const {
      hydrateDialogWidths,
      createDialogWidth,
      NOTE_DIALOG_WIDTH_KEY,
      NOTE_DIALOG_MIN_WIDTH,
      SESSION_DIALOG_WIDTH_KEY,
      NEW_SESSION_DIALOG_WIDTH_KEY,
    } = await importDialogWidth();

    await hydrateDialogWidths();

    expect(getStoreValue.mock.calls.map(([key]) => key)).toEqual([
      NOTE_DIALOG_WIDTH_KEY,
      SESSION_DIALOG_WIDTH_KEY,
      NEW_SESSION_DIALOG_WIDTH_KEY,
    ]);
    // The instance a dialog creates on mount is the one already hydrated.
    const note = createDialogWidth({
      key: NOTE_DIALOG_WIDTH_KEY,
      minWidth: NOTE_DIALOG_MIN_WIDTH,
    });
    expect(note.width).toBe(900);
    expect(note.hydrated).toBe(true);
  });
});
