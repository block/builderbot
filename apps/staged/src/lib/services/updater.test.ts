import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  createUpdaterService,
  initialUpdaterState,
  UPDATER_CHECK_INTERVAL_MS,
  UPDATER_CHECK_TIMEOUT_MS,
  UPDATER_STALE_SETTLE_GRACE_MS,
  type UpdateHandle,
  type UpdaterAdapters,
  type UpdaterEnvironment,
} from './updater';

// ── Test harness ────────────────────────────────────────────────────────────

interface MockedAdapters extends UpdaterAdapters {
  checkForUpdate: ReturnType<typeof vi.fn<() => Promise<UpdateHandle | null>>>;
  confirmInstall: ReturnType<
    typeof vi.fn<(message: string, options: { title: string; kind: 'info' }) => Promise<boolean>>
  >;
  relaunch: ReturnType<typeof vi.fn<() => Promise<void>>>;
  log: ReturnType<typeof vi.fn<(message: string) => void>>;
}

interface Harness {
  adapters: MockedAdapters;
}

function makeHarness(): Harness {
  return {
    adapters: {
      checkForUpdate: vi.fn<() => Promise<UpdateHandle | null>>().mockResolvedValue(null),
      confirmInstall: vi
        .fn<(message: string, options: { title: string; kind: 'info' }) => Promise<boolean>>()
        .mockResolvedValue(false),
      relaunch: vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
      log: vi.fn<(message: string) => void>(),
      setInterval: (handler: () => void, ms: number) => setInterval(handler, ms),
      clearInterval: (handle: unknown) => clearInterval(handle as ReturnType<typeof setInterval>),
      setTimeout: (handler: () => void, ms: number) => setTimeout(handler, ms),
    },
  };
}

const releaseEnv: UpdaterEnvironment = { enabled: true, dev: false, isTauriApp: true };

function makeUpdate(overrides: Partial<UpdateHandle> = {}): UpdateHandle {
  return {
    version: '1.2.3',
    body: 'notes',
    downloadAndInstall: vi.fn().mockResolvedValue(undefined),
    close: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

/** The `close` mock of an update handle built by `makeUpdate`. */
function closeMock(update: UpdateHandle): ReturnType<typeof vi.fn> {
  return update.close as ReturnType<typeof vi.fn>;
}

function makeService(
  harness: Harness,
  env: UpdaterEnvironment = releaseEnv,
  state = initialUpdaterState()
) {
  return createUpdaterService({ state, env, adapters: harness.adapters });
}

/** Flush pending microtasks so async service work settles under fake timers. */
async function flush(times = 10): Promise<void> {
  for (let i = 0; i < times; i++) await Promise.resolve();
}

beforeEach(() => {
  vi.useFakeTimers();
});

// ── Gating ──────────────────────────────────────────────────────────────────

describe('availability gating', () => {
  it.each([
    ['dev builds', { enabled: true, dev: true, isTauriApp: true }],
    ['web-only builds', { enabled: true, dev: false, isTauriApp: false }],
    ['builds without VITE_UPDATER_ENABLED', { enabled: false, dev: false, isTauriApp: true }],
  ] as const)('never checks in %s', async (_label, env) => {
    const harness = makeHarness();
    const service = makeService(harness, env);
    service.start();
    await flush();
    vi.advanceTimersByTime(UPDATER_CHECK_INTERVAL_MS * 3);
    await flush();

    expect(service.available).toBe(false);
    expect(service.state.status).toBe('unavailable');
    expect(harness.adapters.checkForUpdate).not.toHaveBeenCalled();
  });

  it('ignores manual checks when unavailable', async () => {
    const harness = makeHarness();
    const service = makeService(harness, { enabled: false, dev: false, isTauriApp: true });
    service.start();
    await service.checkNow();

    expect(harness.adapters.checkForUpdate).not.toHaveBeenCalled();
  });
});

// ── Scheduling ──────────────────────────────────────────────────────────────

describe('scheduling', () => {
  it('runs a startup check once and periodic checks on the interval', async () => {
    const harness = makeHarness();
    const service = makeService(harness);
    service.start();
    service.start(); // idempotent
    await flush();
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(1);

    vi.advanceTimersByTime(UPDATER_CHECK_INTERVAL_MS);
    await flush();
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(2);

    vi.advanceTimersByTime(UPDATER_CHECK_INTERVAL_MS);
    await flush();
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(3);
  });

  it('stops periodic checks after dispose', async () => {
    const harness = makeHarness();
    const service = makeService(harness);
    service.start();
    await flush();
    service.dispose();

    vi.advanceTimersByTime(UPDATER_CHECK_INTERVAL_MS * 5);
    await flush();
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(1);
  });
});

// ── Deduplication and timeouts ──────────────────────────────────────────────

describe('deduplication', () => {
  it('coalesces checks requested while one is in flight', async () => {
    const harness = makeHarness();
    let resolveCheck!: (u: UpdateHandle | null) => void;
    harness.adapters.checkForUpdate.mockImplementation(
      () => new Promise<UpdateHandle | null>((resolve) => (resolveCheck = resolve))
    );
    const service = makeService(harness);
    service.start();
    await flush();

    // Two more requests while the startup check is pending → one queued re-run.
    void service.checkNow();
    void service.checkNow();
    await flush();
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(1);

    resolveCheck(null);
    await flush();
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(2);

    resolveCheck(null);
    await flush();
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(2);
  });

  it('resolves a coalesced checkNow() only after the queued run settles', async () => {
    const harness = makeHarness();
    let resolveCheck!: (u: UpdateHandle | null) => void;
    harness.adapters.checkForUpdate.mockImplementation(
      () => new Promise<UpdateHandle | null>((resolve) => (resolveCheck = resolve))
    );
    const service = makeService(harness);
    service.start();
    await flush();

    let settled = false;
    const manualCheck = service.checkNow().then(() => (settled = true));
    await flush();
    // Coalesced behind the in-flight startup check: not settled yet.
    expect(settled).toBe(false);

    // Startup check settles → queued run starts; still not settled.
    resolveCheck(null);
    await flush();
    expect(settled).toBe(false);
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(2);

    // Queued run settles → the manual promise resolves.
    resolveCheck(null);
    await manualCheck;
    expect(settled).toBe(true);
  });

  it('a coalesced manual check bypasses prompt suppression in the queued run', async () => {
    const harness = makeHarness();
    const update = makeUpdate();
    let resolveCheck!: (u: UpdateHandle | null) => void;
    harness.adapters.checkForUpdate.mockImplementation(
      () => new Promise<UpdateHandle | null>((resolve) => (resolveCheck = resolve))
    );
    harness.adapters.confirmInstall.mockResolvedValue(false);
    const service = makeService(harness);
    service.start();
    await flush();

    // Queue a manual check behind the in-flight startup check.
    const manualCheck = service.checkNow();
    await flush();

    // Startup check finds the update and prompts (declined).
    resolveCheck(update);
    await flush();
    expect(harness.adapters.confirmInstall).toHaveBeenCalledTimes(1);

    // The queued run inherited the manual trigger, so it re-prompts.
    resolveCheck(update);
    await manualCheck;
    expect(harness.adapters.confirmInstall).toHaveBeenCalledTimes(2);
  });

  it('settles a queued checkNow() promise when the service is disposed', async () => {
    const harness = makeHarness();
    let resolveCheck!: (u: UpdateHandle | null) => void;
    harness.adapters.checkForUpdate.mockImplementation(
      () => new Promise<UpdateHandle | null>((resolve) => (resolveCheck = resolve))
    );
    const service = makeService(harness);
    service.start();
    await flush();

    const manualCheck = service.checkNow();
    await flush();
    service.dispose();
    resolveCheck(null);
    await expect(manualCheck).resolves.toBeUndefined();
    // The queued run never executes after dispose.
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(1);
  });

  it('does not start a new native check while a timed-out check is still settling', async () => {
    const harness = makeHarness();
    const stale = makeUpdate();
    let resolveStale!: (u: UpdateHandle | null) => void;
    harness.adapters.checkForUpdate.mockImplementationOnce(
      () => new Promise<UpdateHandle | null>((resolve) => (resolveStale = resolve))
    );
    const service = makeService(harness);
    service.start();
    await flush();

    vi.advanceTimersByTime(UPDATER_CHECK_TIMEOUT_MS);
    await flush();
    expect(service.state.status).toBe('error');
    expect(service.state.error).toBe('Update check timed out.');

    // A check requested while the original native request is still pending is
    // queued rather than started.
    void service.checkNow();
    await flush();
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(1);

    // Once the stale check settles, its result is discarded (no prompt), its
    // native handle is released, and the queued run proceeds.
    resolveStale(stale);
    await flush();
    expect(harness.adapters.confirmInstall).not.toHaveBeenCalled();
    expect(closeMock(stale)).toHaveBeenCalledTimes(1);
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(2);
  });

  it('resumes checks when a timed-out native check never settles', async () => {
    const harness = makeHarness();
    harness.adapters.checkForUpdate.mockImplementationOnce(
      () => new Promise<UpdateHandle | null>(() => {})
    );
    const service = makeService(harness);
    service.start();
    await flush();

    vi.advanceTimersByTime(UPDATER_CHECK_TIMEOUT_MS);
    await flush();
    void service.checkNow();
    await flush();
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(1);

    // The wait for the hung native promise is bounded: checks resume anyway.
    vi.advanceTimersByTime(UPDATER_STALE_SETTLE_GRACE_MS);
    await flush();
    expect(harness.adapters.checkForUpdate).toHaveBeenCalledTimes(2);
    expect(service.state.status).toBe('up-to-date');
  });
});

// ── Native handle lifecycle ────────────────────────────────────────────────

describe('update handle lifecycle', () => {
  it('bounds the native check with the same timeout as the renderer race', async () => {
    const harness = makeHarness();
    const service = makeService(harness);
    service.start();
    await flush();

    expect(harness.adapters.checkForUpdate).toHaveBeenCalledWith({
      timeoutMs: UPDATER_CHECK_TIMEOUT_MS,
    });
  });

  it('releases the handle when the user declines', async () => {
    const update = makeUpdate();
    const harness = makeHarness();
    harness.adapters.checkForUpdate.mockResolvedValue(update);
    harness.adapters.confirmInstall.mockResolvedValue(false);
    const service = makeService(harness);
    service.start();
    await flush();

    expect(closeMock(update)).toHaveBeenCalledTimes(1);
  });

  it('releases the handle of a repeat check whose prompt is suppressed', async () => {
    const harness = makeHarness();
    const first = makeUpdate();
    const repeat = makeUpdate();
    harness.adapters.checkForUpdate.mockResolvedValueOnce(first).mockResolvedValue(repeat);
    const service = makeService(harness);
    service.start();
    await flush();

    vi.advanceTimersByTime(UPDATER_CHECK_INTERVAL_MS);
    await flush();
    expect(harness.adapters.confirmInstall).toHaveBeenCalledTimes(1);
    expect(closeMock(first)).toHaveBeenCalledTimes(1);
    expect(closeMock(repeat)).toHaveBeenCalledTimes(1);
  });

  it('releases the handle when the install fails', async () => {
    const update = makeUpdate();
    (update.downloadAndInstall as ReturnType<typeof vi.fn>).mockRejectedValue(
      new Error('disk full')
    );
    const harness = makeHarness();
    harness.adapters.checkForUpdate.mockResolvedValue(update);
    harness.adapters.confirmInstall.mockResolvedValue(true);
    const service = makeService(harness);
    service.start();
    await flush();

    expect(closeMock(update)).toHaveBeenCalledTimes(1);
  });

  it('keeps checking when releasing a handle fails', async () => {
    const update = makeUpdate();
    closeMock(update).mockRejectedValue(new Error('resource already closed'));
    const harness = makeHarness();
    harness.adapters.checkForUpdate.mockResolvedValueOnce(update).mockResolvedValue(null);
    harness.adapters.confirmInstall.mockResolvedValue(false);
    const service = makeService(harness);
    service.start();
    await flush();

    vi.advanceTimersByTime(UPDATER_CHECK_INTERVAL_MS);
    await flush();
    expect(service.state.status).toBe('up-to-date');
  });
});

// ── Check outcomes ──────────────────────────────────────────────────────────

describe('check outcomes', () => {
  it('reports up-to-date when no update is available', async () => {
    const harness = makeHarness();
    const service = makeService(harness);
    service.start();
    await flush();

    expect(service.state.status).toBe('up-to-date');
    expect(service.state.availableVersion).toBeNull();
    expect(harness.adapters.confirmInstall).not.toHaveBeenCalled();
  });

  it('exposes version and release notes for an available update', async () => {
    const harness = makeHarness();
    harness.adapters.checkForUpdate.mockResolvedValue(makeUpdate({ version: '2.0.0' }));
    const service = makeService(harness);
    service.start();
    await flush();

    expect(service.state.status).toBe('update-available');
    expect(service.state.availableVersion).toBe('2.0.0');
    expect(service.state.releaseNotes).toBe('notes');
  });

  it('marks a failed check as a recoverable error and keeps checking', async () => {
    const harness = makeHarness();
    harness.adapters.checkForUpdate
      .mockRejectedValueOnce(new Error('network down'))
      .mockResolvedValue(null);
    const service = makeService(harness);
    service.start();
    await flush();

    expect(service.state.status).toBe('error');
    expect(service.state.error).toBe('network down');

    vi.advanceTimersByTime(UPDATER_CHECK_INTERVAL_MS);
    await flush();
    expect(service.state.status).toBe('up-to-date');
    expect(service.state.error).toBeNull();
  });

  it('allows a manual retry after a failed manual check', async () => {
    const harness = makeHarness();
    harness.adapters.checkForUpdate
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValue(null);
    const service = makeService(harness);
    service.start();
    await flush();
    expect(service.state.status).toBe('error');

    await service.checkNow();
    expect(service.state.status).toBe('up-to-date');
  });
});

// ── Prompt and install flow ────────────────────────────────────────────────

describe('prompt and install flow', () => {
  it('does not prompt twice for the same version across background checks', async () => {
    const harness = makeHarness();
    harness.adapters.checkForUpdate.mockResolvedValue(makeUpdate());
    const service = makeService(harness);
    service.start();
    await flush();
    expect(harness.adapters.confirmInstall).toHaveBeenCalledTimes(1);

    vi.advanceTimersByTime(UPDATER_CHECK_INTERVAL_MS);
    await flush();
    expect(harness.adapters.confirmInstall).toHaveBeenCalledTimes(1);
  });

  it('re-prompts a declined version on an explicit manual check', async () => {
    const update = makeUpdate();
    const harness = makeHarness();
    harness.adapters.checkForUpdate.mockResolvedValue(update);
    harness.adapters.confirmInstall.mockResolvedValueOnce(false).mockResolvedValue(true);
    const service = makeService(harness);
    service.start();
    await flush();
    // Declined at startup; a later background check stays suppressed.
    expect(harness.adapters.confirmInstall).toHaveBeenCalledTimes(1);
    expect(update.downloadAndInstall).not.toHaveBeenCalled();

    // A manual check from Settings bypasses suppression and can install.
    await service.checkNow();
    expect(harness.adapters.confirmInstall).toHaveBeenCalledTimes(2);
    expect(update.downloadAndInstall).toHaveBeenCalledTimes(1);
    expect(harness.adapters.relaunch).toHaveBeenCalledTimes(1);
  });

  it('keeps background checks suppressed after a declined manual re-prompt', async () => {
    const harness = makeHarness();
    harness.adapters.checkForUpdate.mockResolvedValue(makeUpdate());
    harness.adapters.confirmInstall.mockResolvedValue(false);
    const service = makeService(harness);
    service.start();
    await flush();
    await service.checkNow();
    expect(harness.adapters.confirmInstall).toHaveBeenCalledTimes(2);

    // Background checks after the manual decline still do not re-prompt.
    vi.advanceTimersByTime(UPDATER_CHECK_INTERVAL_MS);
    await flush();
    expect(harness.adapters.confirmInstall).toHaveBeenCalledTimes(2);
  });

  it('prompts again when a newer version appears', async () => {
    const harness = makeHarness();
    harness.adapters.checkForUpdate
      .mockResolvedValueOnce(makeUpdate({ version: '1.2.3' }))
      .mockResolvedValue(makeUpdate({ version: '1.2.4' }));
    const service = makeService(harness);
    service.start();
    await flush();

    vi.advanceTimersByTime(UPDATER_CHECK_INTERVAL_MS);
    await flush();
    expect(harness.adapters.confirmInstall).toHaveBeenCalledTimes(2);
  });

  it('does not install when the user declines, and stays usable', async () => {
    const update = makeUpdate();
    const harness = makeHarness();
    harness.adapters.checkForUpdate.mockResolvedValue(update);
    harness.adapters.confirmInstall.mockResolvedValue(false);
    const service = makeService(harness);
    service.start();
    await flush();

    expect(update.downloadAndInstall).not.toHaveBeenCalled();
    expect(harness.adapters.relaunch).not.toHaveBeenCalled();
    expect(service.state.status).toBe('update-available');
  });

  it('downloads, installs, and relaunches when the user accepts', async () => {
    const update = makeUpdate();
    const harness = makeHarness();
    harness.adapters.checkForUpdate.mockResolvedValue(update);
    harness.adapters.confirmInstall.mockResolvedValue(true);
    const service = makeService(harness);
    service.start();
    await flush();

    expect(harness.adapters.confirmInstall).toHaveBeenCalledWith(
      'Staged 1.2.3 is available. Install now? The app will restart once finished.',
      { title: 'Update Available', kind: 'info' }
    );
    expect(update.downloadAndInstall).toHaveBeenCalledTimes(1);
    expect(service.state.status).toBe('restart-required');
    expect(harness.adapters.relaunch).toHaveBeenCalledTimes(1);
  });

  it('reports a recoverable error when download/install fails', async () => {
    const update = makeUpdate();
    (update.downloadAndInstall as ReturnType<typeof vi.fn>).mockRejectedValue(
      new Error('disk full')
    );
    const harness = makeHarness();
    harness.adapters.checkForUpdate.mockResolvedValue(update);
    harness.adapters.confirmInstall.mockResolvedValue(true);
    const service = makeService(harness);
    service.start();
    await flush();

    expect(service.state.status).toBe('error');
    expect(service.state.error).toBe('disk full');
    expect(harness.adapters.relaunch).not.toHaveBeenCalled();
  });

  it('does not relaunch when disposed mid-install', async () => {
    const harness = makeHarness();
    let resolveInstall!: () => void;
    const update = makeUpdate();
    (update.downloadAndInstall as ReturnType<typeof vi.fn>).mockImplementation(
      () => new Promise<void>((resolve) => (resolveInstall = resolve))
    );
    harness.adapters.checkForUpdate.mockResolvedValue(update);
    harness.adapters.confirmInstall.mockResolvedValue(true);
    const service = makeService(harness);
    service.start();
    await flush();
    expect(service.state.status).toBe('installing');

    service.dispose();
    resolveInstall();
    await flush();
    expect(harness.adapters.relaunch).not.toHaveBeenCalled();
  });
});
