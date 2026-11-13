import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * The `stagedctl project open` listener. What it has to get right: the
 * request reaches this window alone, a request parked before the listener was
 * live is still picked up, no request is acted on or answered twice, and
 * requests that overlap leave the window on the one asked for last.
 */

type Request = { requestId: string; projectId: string };
type Answer = 'opened' | 'dialogOpen' | 'diffCommentUnsaved' | 'projectNotFound';
type ListenOptions = { onEstablished?: () => void };

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

let windowEvent: string | undefined;
let windowCallback: ((request: Request) => void) | undefined;
let windowOptions: ListenOptions | undefined;
let transportUnlisten: ReturnType<typeof vi.fn>;
let listenToEvent: ReturnType<typeof vi.fn>;
let listPendingProjectOpens: ReturnType<typeof vi.fn<() => Promise<Request[]>>>;
let answerProjectOpen: ReturnType<
  typeof vi.fn<(requestId: string, outcome: Answer) => Promise<void>>
>;
let openProjectExternally: ReturnType<typeof vi.fn<(projectId: string) => Promise<Answer>>>;

async function startListening() {
  const { listenForProjectOpens } = await import('./projectOpenListener');
  return listenForProjectOpens();
}

/** The backend's `emit_to` reaching this window. */
function emit(request: Request): void {
  if (!windowCallback) throw new Error('no window listener registered');
  windowCallback(request);
}

/** The registration landing: the listener is live. */
function establish(): void {
  if (!windowOptions?.onEstablished) throw new Error('no established hook registered');
  windowOptions.onEstablished();
}

/** Let every already-settled promise run its continuations. */
async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  vi.resetModules();
  windowEvent = undefined;
  windowCallback = undefined;
  windowOptions = undefined;
  transportUnlisten = vi.fn();
  listenToEvent = vi.fn();
  listPendingProjectOpens = vi.fn<() => Promise<Request[]>>().mockResolvedValue([]);
  answerProjectOpen = vi
    .fn<(requestId: string, outcome: Answer) => Promise<void>>()
    .mockResolvedValue(undefined);
  openProjectExternally = vi
    .fn<(projectId: string) => Promise<Answer>>()
    .mockResolvedValue('opened');

  vi.doMock('../transport', () => ({
    listenToEvent,
    listenToWindowEvent: (
      event: string,
      callback: (request: Request) => void,
      opts?: ListenOptions
    ) => {
      windowEvent = event;
      windowCallback = callback;
      windowOptions = opts;
      return transportUnlisten;
    },
  }));
  vi.doMock('../commands', () => ({ listPendingProjectOpens, answerProjectOpen }));
  vi.doMock('../features/layout/navigation.svelte', () => ({ openProjectExternally }));
});

afterEach(() => {
  vi.doUnmock('../transport');
  vi.doUnmock('../commands');
  vi.doUnmock('../features/layout/navigation.svelte');
  vi.restoreAllMocks();
});

describe('listenForProjectOpens', () => {
  it('listens for the backend’s event, window-scoped', async () => {
    await startListening();

    // Mirrors `PROJECT_OPEN_REQUESTED_EVENT` in `project_open/control.rs`,
    // hard-coded rather than read from the module.
    expect(windowEvent).toBe('project-open-requested');
    // An any-target listener would navigate every window for one request.
    expect(listenToEvent).not.toHaveBeenCalled();
  });

  it('opens the project a request names and answers with what happened', async () => {
    await startListening();

    emit({ requestId: 'r1', projectId: 'p1' });
    await settle();

    expect(openProjectExternally).toHaveBeenCalledExactlyOnceWith('p1');
    expect(answerProjectOpen).toHaveBeenCalledExactlyOnceWith('r1', 'opened');
  });

  it('passes each refusal to the backend unchanged', async () => {
    await startListening();

    const refusals: Answer[] = ['dialogOpen', 'diffCommentUnsaved', 'projectNotFound'];
    for (const [index, refusal] of refusals.entries()) {
      openProjectExternally.mockResolvedValueOnce(refusal);
      emit({ requestId: `r${index}`, projectId: 'p1' });
      await settle();
      expect(answerProjectOpen).toHaveBeenLastCalledWith(`r${index}`, refusal);
    }
    expect(answerProjectOpen).toHaveBeenCalledTimes(refusals.length);
  });

  it('takes the snapshot only once the listener is live', async () => {
    listPendingProjectOpens.mockResolvedValue([{ requestId: 'r1', projectId: 'p1' }]);
    await startListening();
    await settle();

    // Taken any earlier, a request parked between the snapshot and the
    // registration landing would be in neither.
    expect(listPendingProjectOpens).not.toHaveBeenCalled();

    establish();
    await settle();

    // Parked before this window was listening: the event never arrived.
    expect(listPendingProjectOpens).toHaveBeenCalledTimes(1);
    expect(openProjectExternally).toHaveBeenCalledExactlyOnceWith('p1');
    expect(answerProjectOpen).toHaveBeenCalledExactlyOnceWith('r1', 'opened');
  });

  it('acts once on a request both the event and the snapshot deliver', async () => {
    const snapshot = deferred<Request[]>();
    listPendingProjectOpens.mockReturnValue(snapshot.promise);
    await startListening();

    // The event lands while the snapshot is in flight, and is answered
    // before the snapshot, read before that answer, comes back.
    establish();
    emit({ requestId: 'r1', projectId: 'p1' });
    await settle();
    expect(answerProjectOpen).toHaveBeenCalledTimes(1);

    snapshot.resolve([{ requestId: 'r1', projectId: 'p1' }]);
    await settle();

    expect(openProjectExternally).toHaveBeenCalledTimes(1);
    expect(answerProjectOpen).toHaveBeenCalledTimes(1);
  });

  it('acts once on a request the snapshot delivers before the event', async () => {
    listPendingProjectOpens.mockResolvedValue([{ requestId: 'r1', projectId: 'p1' }]);
    await startListening();

    establish();
    await settle();
    emit({ requestId: 'r1', projectId: 'p1' });
    await settle();

    expect(openProjectExternally).toHaveBeenCalledTimes(1);
    expect(answerProjectOpen).toHaveBeenCalledTimes(1);
  });

  it('acts once on an event delivered twice', async () => {
    await startListening();

    emit({ requestId: 'r1', projectId: 'p1' });
    emit({ requestId: 'r1', projectId: 'p1' });
    await settle();

    expect(openProjectExternally).toHaveBeenCalledTimes(1);
    expect(answerProjectOpen).toHaveBeenCalledTimes(1);
  });

  it('holds a request back until the one before it has been answered', async () => {
    const first = deferred<Answer>();
    openProjectExternally.mockReturnValueOnce(first.promise);
    await startListening();

    // The first is waiting on a diff comment being saved, say.
    emit({ requestId: 'r1', projectId: 'p1' });
    emit({ requestId: 'r2', projectId: 'p2' });
    await settle();

    // Navigating for the second now would be undone when the first lands.
    expect(openProjectExternally).toHaveBeenCalledExactlyOnceWith('p1');

    first.resolve('opened');
    await settle();

    expect(openProjectExternally.mock.calls).toEqual([['p1'], ['p2']]);
    expect(answerProjectOpen.mock.calls).toEqual([
      ['r1', 'opened'],
      ['r2', 'opened'],
    ]);
  });

  it('drops a waiting request that a newer one replaced', async () => {
    const first = deferred<Answer>();
    openProjectExternally.mockReturnValueOnce(first.promise);
    await startListening();

    emit({ requestId: 'r1', projectId: 'p1' });
    emit({ requestId: 'r2', projectId: 'p2' });
    emit({ requestId: 'r3', projectId: 'p3' });
    first.resolve('opened');
    await settle();

    // The backend told r2's caller it was superseded when r3 was parked, so
    // the window neither navigates for it nor answers it.
    expect(openProjectExternally.mock.calls).toEqual([['p1'], ['p3']]);
    expect(answerProjectOpen.mock.calls).toEqual([
      ['r1', 'opened'],
      ['r3', 'opened'],
    ]);
  });

  it('keeps going after an answer the backend refuses', async () => {
    const consoleWarn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    answerProjectOpen.mockRejectedValueOnce(
      new Error('that project-open request is no longer pending')
    );
    await startListening();

    // Its caller timed out, or a newer request superseded it, while the
    // window was busy. Caught, so it can't escape as an unhandled rejection.
    emit({ requestId: 'r1', projectId: 'p1' });
    await settle();
    expect(consoleWarn).toHaveBeenCalledTimes(1);

    emit({ requestId: 'r2', projectId: 'p2' });
    await settle();
    expect(answerProjectOpen).toHaveBeenLastCalledWith('r2', 'opened');
  });

  it('sends no answer when the open itself fails, and keeps going', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    openProjectExternally.mockRejectedValueOnce(new Error('store down'));
    await startListening();

    emit({ requestId: 'r1', projectId: 'p1' });
    await settle();

    // No answer would be true, so the caller is left to read a timeout.
    expect(answerProjectOpen).not.toHaveBeenCalled();
    expect(consoleError).toHaveBeenCalledTimes(1);

    emit({ requestId: 'r2', projectId: 'p2' });
    await settle();
    expect(answerProjectOpen).toHaveBeenCalledExactlyOnceWith('r2', 'opened');
  });

  it('swallows a snapshot that fails and still hears the event', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    listPendingProjectOpens.mockRejectedValue(new Error('ipc down'));
    await startListening();

    establish();
    await settle();
    expect(consoleError).toHaveBeenCalledTimes(1);

    emit({ requestId: 'r1', projectId: 'p1' });
    await settle();
    expect(answerProjectOpen).toHaveBeenCalledExactlyOnceWith('r1', 'opened');
  });

  it('tears the listener down and takes nothing afterwards', async () => {
    const first = deferred<Answer>();
    const snapshot = deferred<Request[]>();
    openProjectExternally.mockReturnValueOnce(first.promise);
    listPendingProjectOpens.mockReturnValue(snapshot.promise);
    const unlisten = await startListening();
    establish();
    emit({ requestId: 'r1', projectId: 'p1' });
    emit({ requestId: 'r2', projectId: 'p2' });
    await settle();

    unlisten();
    expect(transportUnlisten).toHaveBeenCalledTimes(1);

    // A late event, a late snapshot and the request that was waiting its
    // turn are all left alone. The one already under way still answers.
    emit({ requestId: 'r3', projectId: 'p3' });
    snapshot.resolve([{ requestId: 'r4', projectId: 'p4' }]);
    first.resolve('opened');
    await settle();

    expect(openProjectExternally).toHaveBeenCalledExactlyOnceWith('p1');
    expect(answerProjectOpen).toHaveBeenCalledExactlyOnceWith('r1', 'opened');
  });
});
