import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * The window's reveal. What it has to get right: the backend is told only
 * after the window is really on screen, a failed report does not undo a
 * reveal that worked, and web mode, which has no native window, reports
 * nothing.
 */

/** A promise the test settles by hand. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

let show: ReturnType<typeof vi.fn<() => Promise<void>>>;
let noteWindowRevealed: ReturnType<typeof vi.fn<() => Promise<void>>>;

/** Load the module with the transport reporting desktop or web mode. */
async function load(isTauri: boolean) {
  vi.doMock('../transport', () => ({ isTauri, getWindowSync: () => ({ show }) }));
  vi.doMock('../commands', () => ({ noteWindowRevealed }));
  return import('./windowReveal');
}

beforeEach(() => {
  vi.resetModules();
  show = vi.fn<() => Promise<void>>().mockResolvedValue(undefined);
  noteWindowRevealed = vi.fn<() => Promise<void>>().mockResolvedValue(undefined);
});

afterEach(() => {
  vi.doUnmock('../transport');
  vi.doUnmock('../commands');
  vi.restoreAllMocks();
});

describe('revealWindow', () => {
  it('shows the window and then tells the backend it is revealed', async () => {
    const { revealWindow } = await load(true);

    await revealWindow();

    expect(show).toHaveBeenCalledTimes(1);
    // Mirrors `note_window_revealed` in `app_lifecycle/window_reveal.rs`.
    expect(noteWindowRevealed).toHaveBeenCalledTimes(1);
    expect(show.mock.invocationCallOrder[0]).toBeLessThan(
      noteWindowRevealed.mock.invocationCallOrder[0]
    );
  });

  it('does not report the reveal until the window is on screen', async () => {
    // The backend raises a window it is told is revealed. Told early, it
    // would show the window before the frontend had.
    const shown = deferred<void>();
    show.mockReturnValue(shown.promise);
    const { revealWindow } = await load(true);

    const revealing = revealWindow();
    await Promise.resolve();
    expect(noteWindowRevealed).not.toHaveBeenCalled();

    shown.resolve();
    await revealing;
    expect(noteWindowRevealed).toHaveBeenCalledTimes(1);
  });

  it('settles only once the report has', async () => {
    const reported = deferred<void>();
    noteWindowRevealed.mockReturnValue(reported.promise);
    const { revealWindow } = await load(true);
    let settled = false;

    const revealing = revealWindow().then(() => {
      settled = true;
    });
    await vi.waitFor(() => expect(noteWindowRevealed).toHaveBeenCalled());
    expect(settled).toBe(false);

    reported.resolve();
    await revealing;
    expect(settled).toBe(true);
  });

  it('reports nothing when the window could not be shown', async () => {
    show.mockRejectedValue(new Error('no window'));
    const { revealWindow } = await load(true);

    await expect(revealWindow()).rejects.toThrow('no window');

    expect(noteWindowRevealed).not.toHaveBeenCalled();
  });

  it('still resolves when the report fails', async () => {
    // The window is on screen all the same, and the rest of the mount must
    // go on.
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    noteWindowRevealed.mockRejectedValue(new Error('ipc down'));
    const { revealWindow } = await load(true);

    await expect(revealWindow()).resolves.toBeUndefined();

    expect(show).toHaveBeenCalledTimes(1);
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it('reports nothing in web mode', async () => {
    // The web dispatch refuses the command: a browser tab is not a native
    // window the backend could raise.
    const { revealWindow } = await load(false);

    await revealWindow();

    expect(show).toHaveBeenCalledTimes(1);
    expect(noteWindowRevealed).not.toHaveBeenCalled();
  });
});
