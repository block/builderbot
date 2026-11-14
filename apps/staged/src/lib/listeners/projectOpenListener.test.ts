import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * The `stagedctl project open` listener. What it has to get right: the
 * request reaches this window alone, a request parked before the listener was
 * live is still picked up, no request is acted on or answered twice,
 * requests that overlap leave the window on the one asked for last, the
 * window is left alone for a request whose caller was told it failed, and a
 * request found at startup is dealt with before `startupHandled` settles.
 */

type Request = { requestId: string; projectId: string };
type Answer = 'opened' | 'dialogOpen' | 'diffCommentUnsaved' | 'projectNotFound';
/** What the navigation helper resolves to: an answer, or nothing to answer. */
type Result = Answer | 'withdrawn';
/** The helper's second argument: asked right before it navigates. */
type StillWanted = () => Promise<boolean>;
type OpenExternally = (projectId: string, stillWanted?: StillWanted) => Promise<Result>;
type ListenOptions = {
  onEstablished?: () => void;
  onRegistrationFailed?: (error: unknown) => void;
};

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
let openProjectExternally: ReturnType<typeof vi.fn<OpenExternally>>;
/** Stands in for `selectProject` when a test supplies its own helper. */
let navigate: ReturnType<typeof vi.fn<(projectId: string) => void>>;

async function startListening() {
  const { listenForProjectOpens } = await import('./projectOpenListener');
  return listenForProjectOpens();
}

/** The project ids the helper was asked to open, in order. */
function opened(): string[] {
  return openProjectExternally.mock.calls.map(([projectId]) => projectId);
}

/**
 * A helper shaped like the real one: it waits (on the project list or a diff
 * comment being saved, say), then asks whether the request is still wanted,
 * and navigates only if it is.
 */
function helperThatWaits(wait: Promise<void>): OpenExternally {
  return async (projectId, stillWanted) => {
    await wait;
    if (stillWanted && !(await stillWanted())) return 'withdrawn';
    navigate(projectId);
    return 'opened';
  };
}

/** Whether `promise` has settled by the time the queued work has run. */
async function hasSettled(promise: Promise<void>): Promise<boolean> {
  let settled = false;
  void promise.then(() => (settled = true));
  await settle();
  return settled;
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
  openProjectExternally = vi.fn<OpenExternally>().mockResolvedValue('opened');
  navigate = vi.fn<(projectId: string) => void>();

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
  vi.useRealTimers();
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

    expect(opened()).toEqual(['p1']);
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
    expect(opened()).toEqual(['p1']);
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
    expect(opened()).toEqual(['p1']);

    first.resolve('opened');
    await settle();

    expect(opened()).toEqual(['p1', 'p2']);
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
    expect(opened()).toEqual(['p1', 'p3']);
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
    const { stop } = await startListening();
    establish();
    emit({ requestId: 'r1', projectId: 'p1' });
    emit({ requestId: 'r2', projectId: 'p2' });
    await settle();

    stop();
    expect(transportUnlisten).toHaveBeenCalledTimes(1);

    // A late event, a late snapshot and the request that was waiting its
    // turn are all left alone. The one already under way still answers.
    emit({ requestId: 'r3', projectId: 'p3' });
    snapshot.resolve([{ requestId: 'r4', projectId: 'p4' }]);
    first.resolve('opened');
    await settle();

    expect(opened()).toEqual(['p1']);
    expect(answerProjectOpen).toHaveBeenCalledExactlyOnceWith('r1', 'opened');
  });

  describe('a request whose caller was told it failed', () => {
    it('gives the helper a check that asks the backend for this request', async () => {
      await startListening();
      emit({ requestId: 'r1', projectId: 'p1' });
      await settle();
      const stillWanted = openProjectExternally.mock.calls[0][1];
      if (!stillWanted) throw new Error('the helper was given no check');
      listPendingProjectOpens.mockClear();

      // Still the request the backend holds for this window.
      listPendingProjectOpens.mockResolvedValueOnce([{ requestId: 'r1', projectId: 'p1' }]);
      await expect(stillWanted()).resolves.toBe(true);
      // Its caller timed out or went away: the backend holds nothing.
      listPendingProjectOpens.mockResolvedValueOnce([]);
      await expect(stillWanted()).resolves.toBe(false);
      // A newer request for the same project replaced it: matched on the
      // request id, never on the project.
      listPendingProjectOpens.mockResolvedValueOnce([{ requestId: 'r2', projectId: 'p1' }]);
      await expect(stillWanted()).resolves.toBe(false);
      // Asked afresh each time: an earlier reply says nothing about now.
      expect(listPendingProjectOpens).toHaveBeenCalledTimes(3);
    });

    it('sends no answer for a request the helper found withdrawn', async () => {
      const consoleWarn = vi.spyOn(console, 'warn').mockImplementation(() => {});
      openProjectExternally.mockResolvedValueOnce('withdrawn');
      await startListening();

      emit({ requestId: 'r1', projectId: 'p1' });
      await settle();

      // The backend would refuse it, and 'withdrawn' is not an answer it knows.
      expect(answerProjectOpen).not.toHaveBeenCalled();
      expect(consoleWarn).toHaveBeenCalledTimes(1);

      emit({ requestId: 'r2', projectId: 'p2' });
      await settle();
      expect(answerProjectOpen).toHaveBeenCalledExactlyOnceWith('r2', 'opened');
    });

    it('does not navigate for a request withdrawn while the helper was waiting', async () => {
      vi.spyOn(console, 'warn').mockImplementation(() => {});
      const wait = deferred<void>();
      openProjectExternally.mockImplementation(helperThatWaits(wait.promise));
      listPendingProjectOpens.mockResolvedValue([{ requestId: 'r1', projectId: 'p1' }]);
      await startListening();

      emit({ requestId: 'r1', projectId: 'p1' });
      await settle();
      expect(navigate).not.toHaveBeenCalled();

      // The backend's wait ran out during the helper's: the caller has been
      // told `timedOut` and the request is gone.
      listPendingProjectOpens.mockResolvedValue([]);
      wait.resolve();
      await settle();

      expect(navigate).not.toHaveBeenCalled();
      expect(answerProjectOpen).not.toHaveBeenCalled();
    });

    it('navigates for a request still pending when the helper’s wait ends', async () => {
      const wait = deferred<void>();
      openProjectExternally.mockImplementation(helperThatWaits(wait.promise));
      listPendingProjectOpens.mockResolvedValue([{ requestId: 'r1', projectId: 'p1' }]);
      await startListening();

      emit({ requestId: 'r1', projectId: 'p1' });
      await settle();
      wait.resolve();
      await settle();

      expect(navigate).toHaveBeenCalledExactlyOnceWith('p1');
      expect(answerProjectOpen).toHaveBeenCalledExactlyOnceWith('r1', 'opened');
    });

    it('does not navigate for a request superseded while the helper was waiting', async () => {
      vi.spyOn(console, 'warn').mockImplementation(() => {});
      const firstWait = deferred<void>();
      openProjectExternally
        .mockImplementationOnce(helperThatWaits(firstWait.promise))
        .mockImplementation(helperThatWaits(Promise.resolve()));
      listPendingProjectOpens.mockResolvedValue([{ requestId: 'r1', projectId: 'p1' }]);
      await startListening();

      emit({ requestId: 'r1', projectId: 'p1' });
      await settle();

      // A newer request replaces it in the backend, whose caller of r1 now
      // reads `superseded`, and reaches the window while r1 is still waiting.
      listPendingProjectOpens.mockResolvedValue([{ requestId: 'r2', projectId: 'p2' }]);
      emit({ requestId: 'r2', projectId: 'p2' });
      firstWait.resolve();
      await settle();

      // Navigating for r1 would leave the window on p1 if r2 were then
      // refused, after r1's caller was told otherwise.
      expect(navigate.mock.calls).toEqual([['p2']]);
      expect(answerProjectOpen).toHaveBeenCalledExactlyOnceWith('r2', 'opened');
    });

    it('does not navigate for a request withdrawn while it waited its turn', async () => {
      vi.spyOn(console, 'warn').mockImplementation(() => {});
      const firstWait = deferred<void>();
      openProjectExternally
        .mockImplementationOnce(helperThatWaits(firstWait.promise))
        .mockImplementation(helperThatWaits(Promise.resolve()));
      listPendingProjectOpens.mockResolvedValue([{ requestId: 'r1', projectId: 'p1' }]);
      await startListening();

      // r2 replaces r1 in the backend and queues behind it in the window.
      emit({ requestId: 'r1', projectId: 'p1' });
      emit({ requestId: 'r2', projectId: 'p2' });
      await settle();
      expect(opened()).toEqual(['p1']);

      // r2's caller gives up before r1's wait ends, so before r2's turn.
      listPendingProjectOpens.mockResolvedValue([]);
      firstWait.resolve();
      await settle();

      // Both were taken and both had their turn; neither moved the window.
      expect(opened()).toEqual(['p1', 'p2']);
      expect(navigate).not.toHaveBeenCalled();
      expect(answerProjectOpen).not.toHaveBeenCalled();
    });

    it('sends no answer and navigates nowhere when the check itself fails', async () => {
      const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
      openProjectExternally.mockImplementationOnce(helperThatWaits(Promise.resolve()));
      await startListening();

      listPendingProjectOpens.mockRejectedValueOnce(new Error('ipc down'));
      emit({ requestId: 'r1', projectId: 'p1' });
      await settle();

      // Not known to be wanted, so not navigated: the caller reads a timeout.
      expect(navigate).not.toHaveBeenCalled();
      expect(answerProjectOpen).not.toHaveBeenCalled();
      expect(consoleError).toHaveBeenCalledTimes(1);
    });
  });

  describe('startupHandled', () => {
    it('settles only once a request parked before the window loaded is answered', async () => {
      const open = deferred<Result>();
      const answer = deferred<void>();
      openProjectExternally.mockReturnValueOnce(open.promise);
      answerProjectOpen.mockReturnValueOnce(answer.promise);
      listPendingProjectOpens.mockResolvedValue([{ requestId: 'r1', projectId: 'p1' }]);
      const { startupHandled } = await startListening();
      // What App.svelte chains on it: the launch-resume offer, whose dialog
      // would refuse the request if it opened first.
      const loadResumeOffer = vi.fn();
      void startupHandled.then(loadResumeOffer);

      // Not while the registration is in flight, nor while the project is
      // being opened (it can be waiting on the first project list).
      await settle();
      expect(loadResumeOffer).not.toHaveBeenCalled();
      establish();
      await settle();
      expect(opened()).toEqual(['p1']);
      expect(loadResumeOffer).not.toHaveBeenCalled();

      // Nor until the answer is recorded: until then the request is pending.
      open.resolve('opened');
      await settle();
      expect(answerProjectOpen).toHaveBeenCalledExactlyOnceWith('r1', 'opened');
      expect(loadResumeOffer).not.toHaveBeenCalled();

      answer.resolve();
      await settle();
      expect(loadResumeOffer).toHaveBeenCalledTimes(1);
    });

    it('settles after the snapshot when nothing was parked', async () => {
      const snapshot = deferred<Request[]>();
      listPendingProjectOpens.mockReturnValue(snapshot.promise);
      const { startupHandled } = await startListening();

      establish();
      // Not before the snapshot is back: it may hold a request.
      expect(await hasSettled(startupHandled)).toBe(false);

      snapshot.resolve([]);
      expect(await hasSettled(startupHandled)).toBe(true);
      expect(openProjectExternally).not.toHaveBeenCalled();
    });

    it('waits for a request the event delivered ahead of the snapshot', async () => {
      const open = deferred<Result>();
      const snapshot = deferred<Request[]>();
      openProjectExternally.mockReturnValueOnce(open.promise);
      listPendingProjectOpens.mockReturnValue(snapshot.promise);
      const { startupHandled } = await startListening();

      // The snapshot repeats the request the event already delivered, so it
      // takes nothing new, and the request is still under way.
      establish();
      emit({ requestId: 'r1', projectId: 'p1' });
      snapshot.resolve([{ requestId: 'r1', projectId: 'p1' }]);
      expect(await hasSettled(startupHandled)).toBe(false);

      open.resolve('opened');
      expect(await hasSettled(startupHandled)).toBe(true);
      expect(opened()).toEqual(['p1']);
    });

    it('waits for a request that arrives while the parked one is under way', async () => {
      const first = deferred<Result>();
      const second = deferred<Result>();
      openProjectExternally.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
      listPendingProjectOpens.mockResolvedValue([{ requestId: 'r1', projectId: 'p1' }]);
      const { startupHandled } = await startListening();

      establish();
      await settle();
      emit({ requestId: 'r2', projectId: 'p2' });
      first.resolve('opened');
      expect(await hasSettled(startupHandled)).toBe(false);
      expect(opened()).toEqual(['p1', 'p2']);

      second.resolve('opened');
      expect(await hasSettled(startupHandled)).toBe(true);
    });

    it('settles when the parked request turns out to be withdrawn', async () => {
      vi.spyOn(console, 'warn').mockImplementation(() => {});
      openProjectExternally.mockResolvedValueOnce('withdrawn');
      listPendingProjectOpens.mockResolvedValue([{ requestId: 'r1', projectId: 'p1' }]);
      const { startupHandled } = await startListening();

      establish();

      expect(await hasSettled(startupHandled)).toBe(true);
      expect(answerProjectOpen).not.toHaveBeenCalled();
    });

    it('settles when the snapshot fails', async () => {
      vi.spyOn(console, 'error').mockImplementation(() => {});
      listPendingProjectOpens.mockRejectedValue(new Error('ipc down'));
      const { startupHandled } = await startListening();

      establish();

      expect(await hasSettled(startupHandled)).toBe(true);
    });

    it('settles when the listener could not be registered', async () => {
      const { startupHandled } = await startListening();
      expect(await hasSettled(startupHandled)).toBe(false);

      // No listener, so no snapshot either: nothing will be taken.
      windowOptions?.onRegistrationFailed?.(new Error('no window'));

      expect(await hasSettled(startupHandled)).toBe(true);
      expect(listPendingProjectOpens).not.toHaveBeenCalled();
    });

    it('settles when the listener is stopped first', async () => {
      const { stop, startupHandled } = await startListening();
      expect(await hasSettled(startupHandled)).toBe(false);

      stop();

      expect(await hasSettled(startupHandled)).toBe(true);
    });

    it('settles after the backend’s wait when the open never finishes', async () => {
      vi.useFakeTimers();
      openProjectExternally.mockReturnValueOnce(new Promise<Result>(() => {}));
      listPendingProjectOpens.mockResolvedValue([{ requestId: 'r1', projectId: 'p1' }]);
      const { listenForProjectOpens, STARTUP_HANDLED_TIMEOUT_MS } =
        await import('./projectOpenListener');
      const { startupHandled } = listenForProjectOpens();
      let settled = false;
      void startupHandled.then(() => (settled = true));
      establish();

      // Mirrors `ANSWER_TIMEOUT` in `project_open.rs`: by then the caller
      // has been told `timedOut`, so the offer no longer has to wait.
      expect(STARTUP_HANDLED_TIMEOUT_MS).toBe(15_000);
      await vi.advanceTimersByTimeAsync(STARTUP_HANDLED_TIMEOUT_MS - 1);
      expect(opened()).toEqual(['p1']);
      expect(settled).toBe(false);

      await vi.advanceTimersByTimeAsync(1);
      expect(settled).toBe(true);
    });
  });
});
