/**
 * Stable-channel in-app updater service (core logic).
 *
 * Owns the renderer side of Staged's stable update flow, extracted from
 * `src/App.svelte` (see docs/release-and-updates.md for the release
 * contract):
 *
 *   - availability gating (`VITE_UPDATER_ENABLED`, dev builds, Tauri runtime);
 *   - startup and 15-minute periodic background checks;
 *   - explicit manual checks (for a future Settings entry point);
 *   - in-flight run deduplication and timed-out/stale-result handling — a
 *     timed-out native check does not overlap a second native check, but the
 *     wait for it to settle is bounded so a hung call cannot disable updates;
 *   - releasing every native update handle (`Update` is a Tauri resource);
 *   - per-session prompt suppression for background checks (the same version
 *     is prompted once); explicit manual checks always re-prompt so a user
 *     who declined earlier can still install from Settings;
 *   - consent → download → install → relaunch orchestration;
 *   - structured `[updater]` logging;
 *   - cleanup on app teardown.
 *
 * All native touchpoints (updater plugin, dialog, process relaunch, timers,
 * logging) are injectable adapters so the service is unit-testable without
 * native plugins. The renderer never supplies update URLs or keys — the
 * endpoint and public key are fixed in the native release configuration.
 *
 * This module is rune-free; `updater.svelte.ts` wires it to shared reactive
 * state for the app shell and Settings UI, and `updaterBridge.ts` mirrors that
 * state to windows that do not own the updater.
 */

import { isTauri } from '../transport';

// =============================================================================
// Constants
// =============================================================================

/** Cadence for background update checks. */
export const UPDATER_CHECK_INTERVAL_MS = 15 * 60 * 1000;

/** How long a native update check may take before it is considered timed out. */
export const UPDATER_CHECK_TIMEOUT_MS = 15_000;

/**
 * How long to wait for a timed-out native check to settle before starting
 * another one anyway. Bounds recovery: a native call that never settles must
 * not disable update checks for the rest of the session.
 */
export const UPDATER_STALE_SETTLE_GRACE_MS = 30_000;

// =============================================================================
// Types
// =============================================================================

/**
 * Observable updater status, suitable for a Settings surface.
 *
 * - `unavailable` — a gate condition failed; this build never updates.
 * - `idle` — updater is active but no check has completed yet.
 * - `checking` — a check is in flight.
 * - `up-to-date` — the last check found no newer version.
 * - `update-available` — a newer version exists (prompt shown or deferred).
 * - `installing` — the accepted update is downloading/installing.
 * - `restart-required` — install finished; the app is about to relaunch.
 * - `error` — the last check or install failed (recoverable; checks continue).
 */
export type UpdaterStatus =
  | 'unavailable'
  | 'idle'
  | 'checking'
  | 'up-to-date'
  | 'update-available'
  | 'installing'
  | 'restart-required'
  | 'error';

export interface UpdaterState {
  status: UpdaterStatus;
  /** Version offered by the feed when status is update-available or later. */
  availableVersion: string | null;
  /** Release notes from the feed, when present. */
  releaseNotes: string | null;
  /** Concise user-facing message when status is `error`. */
  error: string | null;
}

export function initialUpdaterState(): UpdaterState {
  return { status: 'unavailable', availableVersion: null, releaseNotes: null, error: null };
}

/**
 * Minimal structural view of `@tauri-apps/plugin-updater`'s `Update`.
 *
 * `Update` extends Tauri's `Resource`: every handle owns an entry in the native
 * resource table that is only freed by `close()`, so the service releases each
 * handle it obtains — including ones it never installs.
 */
export interface UpdateHandle {
  version: string;
  body?: string;
  downloadAndInstall(): Promise<void>;
  close(): Promise<void>;
}

/** Build/runtime gates. All three must allow updating for the service to run. */
export interface UpdaterEnvironment {
  /** `VITE_UPDATER_ENABLED === 'true'` — set only by the official release workflow. */
  enabled: boolean;
  /** `import.meta.env.DEV` — dev builds never contact the updater. */
  dev: boolean;
  /** Running under Tauri (web-only builds never contact the updater). */
  isTauriApp: boolean;
}

/** Injectable native/platform touchpoints. Defaults lazy-import the plugins. */
export interface UpdaterAdapters {
  /**
   * Query the fixed release feed. Resolves null when already up to date.
   * `timeoutMs` bounds the native request itself, so a slow or unresponsive
   * feed fails natively instead of leaving a renderer-side promise pending.
   */
  checkForUpdate(options: { timeoutMs: number }): Promise<UpdateHandle | null>;
  /** Native consent dialog. Resolves true when the user accepts the install. */
  confirmInstall(message: string, options: { title: string; kind: 'info' }): Promise<boolean>;
  /** Relaunch the app after a successful install. */
  relaunch(): Promise<void>;
  /** Structured logging (console + native log file in release builds). */
  log(message: string): void;
  setInterval(handler: () => void, ms: number): unknown;
  clearInterval(handle: unknown): void;
  setTimeout(handler: () => void, ms: number): unknown;
}

type UpdateCheckTrigger = 'startup' | 'interval' | 'manual' | 'queued';

export interface UpdaterServiceOptions {
  /** State record the service mutates (pass reactive state from the app shell). */
  state?: UpdaterState;
  env?: UpdaterEnvironment;
  adapters?: Partial<UpdaterAdapters>;
  checkIntervalMs?: number;
  checkTimeoutMs?: number;
  staleSettleGraceMs?: number;
  /**
   * Called after every state transition with the (mutated) state record. Lets
   * the owning window mirror its status to other windows.
   */
  onStateChange?: (state: UpdaterState) => void;
}

export interface UpdaterService {
  /** The state record this service mutates. */
  readonly state: UpdaterState;
  /** Whether all gate conditions allow updating in this build. */
  readonly available: boolean;
  /** Apply gating, run the startup check, and schedule periodic checks. */
  start(): void;
  /**
   * Explicit manual check (Settings). Resolves when the run settles. When a
   * run is already in flight the request is coalesced into a single queued
   * re-run and the returned promise resolves once that queued run settles.
   * Manual checks re-prompt an available version even if it was already
   * declined this session.
   */
  checkNow(): Promise<void>;
  /** Cancel in-flight work and stop periodic checks. Safe to call twice. */
  dispose(): void;
}

// =============================================================================
// Default adapters
// =============================================================================

function defaultEnvironment(): UpdaterEnvironment {
  return {
    enabled: import.meta.env.VITE_UPDATER_ENABLED === 'true',
    dev: import.meta.env.DEV,
    isTauriApp: isTauri,
  };
}

/**
 * Whether this build may contact the update feed.
 *
 * Exported so the app shell can answer the gate question *before* starting the
 * service — claiming the app-wide updater ownership lease is a Tauri-only
 * command, so non-updating builds must never get that far.
 */
export function isUpdaterAvailable(env: UpdaterEnvironment = defaultEnvironment()): boolean {
  return env.enabled && !env.dev && env.isTauriApp;
}

/**
 * Default `[updater]` logging: console plus the native log file in release
 * builds. Exported for shell-level updater messages (ownership) that happen
 * outside a service instance.
 */
export function logUpdaterMessage(message: string): void {
  console.warn(message);
  void (async () => {
    try {
      const { warn } = await import('@tauri-apps/plugin-log');
      await warn(message);
    } catch {
      // log plugin is optional during local web-only development
    }
  })();
}

function defaultAdapters(): UpdaterAdapters {
  return {
    async checkForUpdate({ timeoutMs }) {
      const { check } = await import('@tauri-apps/plugin-updater');
      return check({ timeout: timeoutMs });
    },
    async confirmInstall(message, options) {
      const { confirm } = await import('@tauri-apps/plugin-dialog');
      return confirm(message, options);
    },
    async relaunch() {
      const { relaunch } = await import('@tauri-apps/plugin-process');
      await relaunch();
    },
    log: logUpdaterMessage,
    setInterval: (handler, ms) => globalThis.setInterval(handler, ms),
    clearInterval: (handle) => globalThis.clearInterval(handle as ReturnType<typeof setInterval>),
    setTimeout: (handler, ms) => globalThis.setTimeout(handler, ms),
  };
}

// =============================================================================
// Service
// =============================================================================

export function createUpdaterService(options: UpdaterServiceOptions = {}): UpdaterService {
  const state = options.state ?? initialUpdaterState();
  const env = options.env ?? defaultEnvironment();
  const adapters: UpdaterAdapters = { ...defaultAdapters(), ...options.adapters };
  const checkIntervalMs = options.checkIntervalMs ?? UPDATER_CHECK_INTERVAL_MS;
  const checkTimeoutMs = options.checkTimeoutMs ?? UPDATER_CHECK_TIMEOUT_MS;
  const staleSettleGraceMs = options.staleSettleGraceMs ?? UPDATER_STALE_SETTLE_GRACE_MS;

  const available = isUpdaterAvailable(env);

  /** Apply a state transition, then notify the `onStateChange` hook. */
  function setState(patch: Partial<UpdaterState>): void {
    Object.assign(state, patch);
    options.onStateChange?.(state);
  }

  let started = false;
  let cancelled = false;
  let intervalHandle: unknown = null;
  // One updater run (check → prompt → install) at a time; runs requested
  // while another is active (or a timed-out check is still settling) are
  // coalesced into a single queued re-run. The queued run remembers whether
  // any coalesced requester was manual (manual runs bypass prompt
  // suppression) and resolves the promises of everyone awaiting it.
  let runInFlight = false;
  let queuedRun: { manual: boolean; promise: Promise<void>; resolve: () => void } | null = null;
  // Native checks that lost the timeout race and have not settled yet. No new
  // native check starts while one is outstanding and its late result is
  // discarded, but the wait is bounded by `staleSettleGraceMs`.
  let staleChecks = 0;
  // Prompt each offered version at most once per app session for background
  // checks; explicit manual checks always re-prompt.
  let lastPromptedUpdateVersion: string | null = null;

  function drainQueuedRun(): void {
    if (!cancelled && queuedRun && !runInFlight && staleChecks === 0) {
      const queued = queuedRun;
      queuedRun = null;
      void runUpdater('queued', { manual: queued.manual }).finally(queued.resolve);
    }
  }

  /**
   * Free an update handle's native resource. Every handle must be released,
   * including ones that are never installed (declined, suppressed, stale), or
   * Tauri's resource table grows with every check that finds an update.
   */
  async function releaseUpdate(update: UpdateHandle | null): Promise<void> {
    if (!update) return;
    try {
      await update.close();
    } catch (error) {
      adapters.log(`[updater] failed to release update handle: ${error}`);
    }
  }

  /**
   * Wait for a timed-out native check to settle before starting another one,
   * discarding (and releasing) whatever it eventually produces. The wait is
   * bounded: if the native promise is still pending after the grace period,
   * checks resume anyway, so a wedged native call cannot disable updates for
   * the rest of the session.
   */
  function trackStaleCheck(checkPromise: Promise<UpdateHandle | null>): void {
    staleChecks += 1;
    let released = false;
    const release = (): void => {
      if (released) return;
      released = true;
      staleChecks -= 1;
      drainQueuedRun();
    };

    void checkPromise
      .then(
        (stale) => {
          adapters.log('[updater] timed-out check settled; stale result ignored');
          return releaseUpdate(stale);
        },
        (error) => {
          adapters.log(`[updater] timed-out check failed while settling: ${error}`);
        }
      )
      .finally(release);

    adapters.setTimeout(() => {
      if (released) return;
      adapters.log(
        `[updater] timed-out check still pending after ${staleSettleGraceMs}ms; resuming checks`
      );
      release();
    }, staleSettleGraceMs);
  }

  async function runUpdater(
    trigger: UpdateCheckTrigger,
    options: { manual?: boolean } = {}
  ): Promise<void> {
    const manual = options.manual ?? trigger === 'manual';
    if (cancelled) return;
    if (runInFlight || staleChecks > 0) {
      if (queuedRun) {
        queuedRun.manual ||= manual;
      } else {
        let resolveQueued!: () => void;
        const promise = new Promise<void>((resolve) => (resolveQueued = resolve));
        queuedRun = { manual, promise, resolve: resolveQueued };
      }
      adapters.log(
        `[updater] ${trigger} check queued because a previous run is still ${
          runInFlight ? 'active' : 'settling after a timeout'
        }`
      );
      return queuedRun.promise;
    }

    runInFlight = true;
    // Released in the finally block, whatever the outcome of this run.
    let update: UpdateHandle | null = null;
    try {
      adapters.log(`[updater] checking for updates (${trigger})`);
      setState({ status: 'checking', error: null });

      const checkPromise = adapters.checkForUpdate({ timeoutMs: checkTimeoutMs });
      const timeoutPromise = new Promise<'timeout'>((resolve) => {
        adapters.setTimeout(() => resolve('timeout'), checkTimeoutMs);
      });
      const outcome = await Promise.race([checkPromise, timeoutPromise]);

      if (outcome === 'timeout') {
        // The native request has not settled; hold further checks (bounded)
        // and discard whatever it eventually produced — stale by then.
        trackStaleCheck(checkPromise);
        adapters.log(`[updater] check timed out after ${checkTimeoutMs}ms`);
        if (!cancelled) {
          setState({ status: 'error', error: 'Update check timed out.' });
        }
        return;
      }

      update = outcome;
      if (cancelled) return;

      if (!update) {
        adapters.log('[updater] no update available');
        setState({ status: 'up-to-date', availableVersion: null, releaseNotes: null });
        return;
      }

      setState({
        status: 'update-available',
        availableVersion: update.version,
        releaseNotes: update.body ?? null,
      });

      // Background checks prompt each version at most once per session, but a
      // manual check is an explicit request: always re-prompt so a user who
      // declined earlier can still install without restarting the app.
      if (!manual && lastPromptedUpdateVersion === update.version) {
        adapters.log(`[updater] update ${update.version} already prompted; skipping`);
        return;
      }

      lastPromptedUpdateVersion = update.version;
      adapters.log(`[updater] update found: ${update.version}`);

      const shouldInstall = await adapters.confirmInstall(
        `Staged ${update.version} is available. Install now? The app will restart once finished.`,
        { title: 'Update Available', kind: 'info' }
      );
      if (!shouldInstall || cancelled) {
        adapters.log(`[updater] install deferred for ${update.version}`);
        return;
      }

      adapters.log('[updater] downloading and installing update');
      setState({ status: 'installing' });
      await update.downloadAndInstall();
      if (cancelled) return;

      setState({ status: 'restart-required' });
      await adapters.relaunch();
    } catch (error) {
      // Update failures are recoverable and must never block the app.
      adapters.log(`[updater] check failed: ${error}`);
      if (!cancelled) {
        setState({
          status: 'error',
          error: error instanceof Error ? error.message : String(error),
        });
      }
    } finally {
      runInFlight = false;
      await releaseUpdate(update);
      drainQueuedRun();
    }
  }

  function start(): void {
    if (started) return;
    started = true;

    adapters.log(
      `[updater] gate check: enabled=${env.enabled} dev=${env.dev} isTauriApp=${env.isTauriApp}`
    );
    if (!available) {
      adapters.log('[updater] skipped because a gate condition was not met');
      setState({ status: 'unavailable' });
      return;
    }

    setState({ status: 'idle' });
    void runUpdater('startup');
    intervalHandle = adapters.setInterval(() => {
      void runUpdater('interval');
    }, checkIntervalMs);
  }

  async function checkNow(): Promise<void> {
    if (!available) {
      adapters.log('[updater] manual check ignored: updater unavailable in this build');
      return;
    }
    await runUpdater('manual');
  }

  function dispose(): void {
    cancelled = true;
    if (intervalHandle !== null) {
      adapters.clearInterval(intervalHandle);
      intervalHandle = null;
    }
    // A queued run will never execute after cancellation; settle its waiters.
    if (queuedRun) {
      queuedRun.resolve();
      queuedRun = null;
    }
  }

  return { state, available, start, checkNow, dispose };
}
