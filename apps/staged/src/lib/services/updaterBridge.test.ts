import { describe, expect, it, vi } from 'vitest';
import type { ListenOptions } from '../transport';
import { initialUpdaterState, type UpdaterState } from './updater';
import {
  createUpdaterMirror,
  createUpdaterOwnerBridge,
  UPDATER_CHECK_REQUEST_EVENT,
  UPDATER_STATE_REQUEST_EVENT,
  UPDATER_STATE_SYNC_EVENT,
  type UpdaterBridgeAdapters,
} from './updaterBridge';

// ── Fake event bus ──────────────────────────────────────────────────────────

interface FakeBus {
  adapters: UpdaterBridgeAdapters;
  emit: ReturnType<typeof vi.fn<(event: string, payload?: unknown) => Promise<void>>>;
  log: ReturnType<typeof vi.fn<(message: string) => void>>;
  /** Deliver an event to every live listener, as another window would. */
  deliver(event: string, payload?: unknown): void;
  listenerCount(event: string): number;
}

function makeBus(): FakeBus {
  const listeners = new Map<string, Set<(payload: unknown) => void>>();
  const emit = vi
    .fn<(event: string, payload?: unknown) => Promise<void>>()
    .mockResolvedValue(undefined);
  const log = vi.fn<(message: string) => void>();

  function listen<T>(event: string, callback: (payload: T) => void, opts?: ListenOptions) {
    const set = listeners.get(event) ?? new Set();
    listeners.set(event, set);
    const handler = callback as (payload: unknown) => void;
    set.add(handler);
    // Like listenToEvent, establishment is always asynchronous.
    queueMicrotask(() => {
      if (set.has(handler)) opts?.onEstablished?.();
    });
    return () => {
      set.delete(handler);
    };
  }

  return {
    adapters: { emit, listen, log },
    emit,
    log,
    deliver(event, payload) {
      for (const handler of listeners.get(event) ?? []) handler(payload);
    },
    listenerCount: (event) => listeners.get(event)?.size ?? 0,
  };
}

async function flush(times = 5): Promise<void> {
  for (let i = 0; i < times; i++) await Promise.resolve();
}

function stateOf(overrides: Partial<UpdaterState> = {}): UpdaterState {
  return { ...initialUpdaterState(), ...overrides };
}

// ── Owner bridge ────────────────────────────────────────────────────────────

describe('owner bridge', () => {
  it('broadcasts a plain copy of the current state', () => {
    const bus = makeBus();
    const state = stateOf({ status: 'update-available', availableVersion: '2.0.0' });
    const bridge = createUpdaterOwnerBridge({
      state,
      checkNow: vi.fn(),
      adapters: bus.adapters,
    });

    bridge.broadcast();

    expect(bus.emit).toHaveBeenCalledWith(UPDATER_STATE_SYNC_EVENT, {
      status: 'update-available',
      availableVersion: '2.0.0',
      releaseNotes: null,
      error: null,
    });
    expect(bus.emit.mock.calls[0][1]).not.toBe(state);
  });

  it('replies to a state request with the current state', () => {
    const bus = makeBus();
    const state = stateOf({ status: 'checking' });
    createUpdaterOwnerBridge({ state, checkNow: vi.fn(), adapters: bus.adapters });

    bus.deliver(UPDATER_STATE_REQUEST_EVENT);

    expect(bus.emit).toHaveBeenCalledWith(
      UPDATER_STATE_SYNC_EVENT,
      expect.objectContaining({ status: 'checking' })
    );
  });

  it('runs a manual check when another window requests one', () => {
    const bus = makeBus();
    const checkNow = vi.fn<() => Promise<void>>().mockResolvedValue(undefined);
    createUpdaterOwnerBridge({ state: stateOf(), checkNow, adapters: bus.adapters });

    bus.deliver(UPDATER_CHECK_REQUEST_EVENT);

    expect(checkNow).toHaveBeenCalledTimes(1);
  });

  it('unlistens and stops broadcasting on dispose', () => {
    const bus = makeBus();
    const checkNow = vi.fn<() => Promise<void>>().mockResolvedValue(undefined);
    const bridge = createUpdaterOwnerBridge({
      state: stateOf(),
      checkNow,
      adapters: bus.adapters,
    });

    bridge.dispose();
    bridge.dispose(); // idempotent
    bus.deliver(UPDATER_CHECK_REQUEST_EVENT);
    bus.deliver(UPDATER_STATE_REQUEST_EVENT);
    bridge.broadcast();

    expect(bus.listenerCount(UPDATER_STATE_REQUEST_EVENT)).toBe(0);
    expect(bus.listenerCount(UPDATER_CHECK_REQUEST_EVENT)).toBe(0);
    expect(checkNow).not.toHaveBeenCalled();
    expect(bus.emit).not.toHaveBeenCalled();
  });

  it('logs a failed emit instead of throwing', async () => {
    const bus = makeBus();
    bus.emit.mockRejectedValue(new Error('bus down'));
    const bridge = createUpdaterOwnerBridge({
      state: stateOf(),
      checkNow: vi.fn(),
      adapters: bus.adapters,
    });

    bridge.broadcast();
    await flush();

    expect(bus.log).toHaveBeenCalledWith(expect.stringContaining('bus down'));
  });
});

// ── Mirror ──────────────────────────────────────────────────────────────────

describe('mirror', () => {
  it("falls back to 'idle' until the owner's state arrives", () => {
    const bus = makeBus();
    const state = stateOf({ availableVersion: 'stale', error: 'stale' });
    createUpdaterMirror({ state, adapters: bus.adapters });

    expect(state).toEqual({
      status: 'idle',
      availableVersion: null,
      releaseNotes: null,
      error: null,
    });
  });

  it('requests the current state once its listener is live', async () => {
    const bus = makeBus();
    createUpdaterMirror({ state: stateOf(), adapters: bus.adapters });

    expect(bus.emit).not.toHaveBeenCalled();
    await flush();

    expect(bus.emit).toHaveBeenCalledTimes(1);
    expect(bus.emit).toHaveBeenCalledWith(UPDATER_STATE_REQUEST_EVENT, undefined);
  });

  it('applies synced state from the owner', () => {
    const bus = makeBus();
    const state = stateOf();
    createUpdaterMirror({ state, adapters: bus.adapters });

    bus.deliver(UPDATER_STATE_SYNC_EVENT, {
      status: 'update-available',
      availableVersion: '2.0.0',
      releaseNotes: 'notes',
      error: null,
    });

    expect(state).toEqual({
      status: 'update-available',
      availableVersion: '2.0.0',
      releaseNotes: 'notes',
      error: null,
    });
  });

  it('routes a manual check to the owner', () => {
    const bus = makeBus();
    const mirror = createUpdaterMirror({ state: stateOf(), adapters: bus.adapters });

    mirror.requestCheck();

    expect(bus.emit).toHaveBeenCalledWith(UPDATER_CHECK_REQUEST_EVENT, undefined);
  });

  it('stops applying state and requesting checks after dispose', async () => {
    const bus = makeBus();
    const state = stateOf();
    const mirror = createUpdaterMirror({ state, adapters: bus.adapters });

    mirror.dispose();
    await flush();
    bus.deliver(UPDATER_STATE_SYNC_EVENT, stateOf({ status: 'checking' }));
    mirror.requestCheck();

    expect(state.status).toBe('idle');
    expect(bus.listenerCount(UPDATER_STATE_SYNC_EVENT)).toBe(0);
    expect(bus.emit).not.toHaveBeenCalled();
  });

  it('round-trips with an owner bridge on a shared bus', async () => {
    const bus = makeBus();
    // Emits from either side are delivered to every window, as Tauri does.
    bus.emit.mockImplementation(async (event, payload) => bus.deliver(event, payload));
    const ownerState = stateOf({ status: 'up-to-date' });
    const checkNow = vi.fn(async () => {
      ownerState.status = 'checking';
      owner.broadcast();
    });
    const owner = createUpdaterOwnerBridge({
      state: ownerState,
      checkNow,
      adapters: bus.adapters,
    });
    const mirrorState = stateOf();
    const mirror = createUpdaterMirror({ state: mirrorState, adapters: bus.adapters });

    await flush();
    expect(mirrorState.status).toBe('up-to-date');

    mirror.requestCheck();
    await flush();
    expect(checkNow).toHaveBeenCalledTimes(1);
    expect(mirrorState.status).toBe('checking');
  });
});
