// flingScroll.ts — the coasting a pointer-driven scroller does after a flick.
//
// DiffModal scrolls the phone's diff panes itself (the diff viewer scrolls
// by transform, and the panes also carry the spine drag), so the browser's
// momentum never applies: a 300 px flick moved the content 300 px and
// stopped. This gives a lift at speed the same coasting a native scroller
// has. The finger's speed is read from its last VELOCITY_WINDOW_MS of
// samples, and the content then keeps moving at that speed, losing a fixed
// share of it every millisecond (FLING_DECAY_PER_MS, UIScrollView's normal
// deceleration rate) until the speed is spent or the content stops moving
// (an edge). A finger that paused before lifting leaves no samples in the
// window, so it flings nothing, as on iOS.
//
// Pure, so the maths is testable: the caller applies each frame's distance
// and reports whether the content moved.

/** The share of the speed kept per millisecond of coasting. */
export const FLING_DECAY_PER_MS = 0.998;
/** Below this speed (px/ms) a lift starts no fling, and a fling stops. */
export const FLING_MIN_VELOCITY = 0.05;
/**
 * The fastest a fling starts (px/ms). A real flick peaks near this; the
 * synthetic swipes of test drivers, delivered as one jump a few
 * milliseconds long, would otherwise read as hundreds of px/ms and throw
 * the content to its far edge.
 */
export const FLING_MAX_VELOCITY = 6;
/**
 * Only the finger's path this recent before its last move sets the speed,
 * and a lift this long after the last move is a finger held still first.
 */
export const VELOCITY_WINDOW_MS = 100;
/**
 * The longest a frame is allowed to stand for. A tab hidden mid-fling gets
 * no frames; when they resume, the content takes up where it was rather
 * than jumping by the whole pause.
 */
export const MAX_FRAME_MS = 64;

export interface PointerSample {
  /** `event.timeStamp`, ms. */
  t: number;
  x: number;
  y: number;
}

export interface Velocity {
  /** px/ms, in the pointer's direction. */
  vx: number;
  vy: number;
}

const STILL: Velocity = { vx: 0, vy: 0 };

/**
 * The pointer's recent path: the down and each move. `velocity(liftT)` is
 * the straight-line speed over the samples within VELOCITY_WINDOW_MS
 * before the last move. It is zero when the lift comes more than the
 * window after that move (the finger stopped, then lifted) or when there
 * is only one sample. The lift itself is not a sample: it sits where the
 * last move left the finger, and counting it would read a lift that
 * follows a sparse move stream as a stop.
 */
export function createVelocityTracker(windowMs = VELOCITY_WINDOW_MS) {
  let samples: PointerSample[] = [];

  function prune(now: number) {
    const cutoff = now - windowMs;
    let drop = 0;
    while (drop < samples.length && samples[drop].t < cutoff) drop += 1;
    if (drop > 0) samples = samples.slice(drop);
  }

  return {
    reset() {
      samples = [];
    },
    push(sample: PointerSample) {
      samples.push(sample);
      prune(sample.t);
    },
    velocity(liftT: number): Velocity {
      if (samples.length < 2) return STILL;
      const last = samples[samples.length - 1];
      if (liftT - last.t > windowMs) return STILL;
      const first = samples[0];
      const dt = last.t - first.t;
      if (dt <= 0) return STILL;
      return { vx: (last.x - first.x) / dt, vy: (last.y - first.y) / dt };
    },
  };
}

export type VelocityTracker = ReturnType<typeof createVelocityTracker>;

/** Whether `velocity` (px/ms) is fast enough to fling, or to keep flinging. */
export function isFlingVelocity(velocity: number, min = FLING_MIN_VELOCITY): boolean {
  return Math.abs(velocity) >= min;
}

/**
 * One frame of coasting: the distance covered over `dtMs` starting at
 * `velocity` and losing `1 - decay` of it per millisecond (the geometric
 * sum), and the velocity left at the end of the frame. Over an unbounded
 * fling the distances sum to `velocity / (1 - decay)`.
 */
export function flingStep(
  velocity: number,
  dtMs: number,
  decay = FLING_DECAY_PER_MS
): { distance: number; velocity: number } {
  const kept = Math.pow(decay, dtMs);
  return { distance: (velocity * (1 - kept)) / (1 - decay), velocity: velocity * kept };
}

export interface FrameScheduler {
  request: (callback: (now: number) => void) => number;
  cancel: (id: number) => void;
}

export interface FlingHandle {
  cancel(): void;
}

/**
 * Coast from `velocity` (px/ms), calling `apply` with each frame's
 * distance. `apply` returns whether the content moved; a frame that moves
 * nothing (an edge) ends the fling, as does the speed dropping under
 * FLING_MIN_VELOCITY. A frame with no time since the last is skipped, not
 * applied, so only those two end it. Returns null when `velocity` is too slow to start;
 * faster than FLING_MAX_VELOCITY starts at that. The first frame only
 * takes the clock; the content moves from the second.
 */
export function startFling(
  velocity: number,
  apply: (distance: number) => boolean,
  frames: FrameScheduler = {
    request: (callback) => requestAnimationFrame(callback),
    cancel: (id) => cancelAnimationFrame(id),
  }
): FlingHandle | null {
  if (!isFlingVelocity(velocity)) return null;
  let remaining = Math.sign(velocity) * Math.min(Math.abs(velocity), FLING_MAX_VELOCITY);
  let last: number | null = null;
  let id = 0;
  const tick = (now: number) => {
    if (last === null) {
      last = now;
      id = frames.request(tick);
      return;
    }
    const dt = Math.min(Math.max(now - last, 0), MAX_FRAME_MS);
    last = now;
    // Two frames on one timestamp (a throttled or just-shown tab) cover no
    // distance; applying nothing would read as an edge and end the fling.
    if (dt === 0) {
      id = frames.request(tick);
      return;
    }
    const step = flingStep(remaining, dt);
    remaining = step.velocity;
    if (apply(step.distance) && isFlingVelocity(remaining)) id = frames.request(tick);
  };
  id = frames.request(tick);
  return { cancel: () => frames.cancel(id) };
}
