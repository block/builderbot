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

  it('clamps above the window maximum down to a gutter on each side', async () => {
    const { createDialogWidth } = await importDialogWidth();

    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    width.set(5000);

    // 1200 window − 32px gutter per side.
    expect(width.width).toBe(1136);
    expect(width.maxWidth).toBe(1136);
  });

  it('keeps the minimum when the window is narrower than it', async () => {
    vi.stubGlobal('window', { innerWidth: 500 });
    const { createDialogWidth } = await importDialogWidth();

    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    width.set(5000);

    expect(width.maxWidth).toBe(700);
    expect(width.width).toBe(700);
  });

  it('hydrates from the store, clamping the saved value', async () => {
    savedWidth = 5000;
    const { createDialogWidth } = await importDialogWidth();

    const width = createDialogWidth({ key: 'test-width', minWidth: 700 });
    await width.ensureHydrated();

    expect(getStoreValue).toHaveBeenCalledWith('test-width');
    expect(width.width).toBe(1136);
    expect(width.hydrated).toBe(true);
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
