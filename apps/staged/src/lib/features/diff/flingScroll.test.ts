import { describe, expect, it, vi } from 'vitest';
import {
  createVelocityTracker,
  FLING_DECAY_PER_MS,
  FLING_MAX_VELOCITY,
  FLING_MIN_VELOCITY,
  flingStep,
  isFlingVelocity,
  MAX_FRAME_MS,
  startFling,
  VELOCITY_WINDOW_MS,
  type FrameScheduler,
} from './flingScroll';

describe('createVelocityTracker', () => {
  it('reads the speed over the samples in the window', () => {
    const tracker = createVelocityTracker();
    tracker.push({ t: 1000, x: 0, y: 100 });
    tracker.push({ t: 1050, x: 10, y: 60 });
    tracker.push({ t: 1100, x: 20, y: 20 });
    expect(tracker.velocity(1100)).toEqual({ vx: 0.2, vy: -0.8 });
  });

  it('drops samples older than the window, so an early slow start does not count', () => {
    const tracker = createVelocityTracker();
    tracker.push({ t: 0, x: 0, y: 0 });
    tracker.push({ t: 500, x: 0, y: 10 });
    tracker.push({ t: 550, x: 0, y: 60 });
    tracker.push({ t: 600, x: 0, y: 110 });
    expect(tracker.velocity(600).vy).toBe(1);
  });

  it('is still after a pause before the lift', () => {
    const tracker = createVelocityTracker();
    tracker.push({ t: 0, x: 0, y: 0 });
    tracker.push({ t: 50, x: 0, y: 100 });
    // The finger stayed put from 50 to the lift at 50 + window + 1.
    expect(tracker.velocity(50 + VELOCITY_WINDOW_MS + 1)).toEqual({ vx: 0, vy: 0 });
  });

  it('is still with one sample or no time between them', () => {
    const tracker = createVelocityTracker();
    tracker.push({ t: 10, x: 5, y: 5 });
    expect(tracker.velocity(10)).toEqual({ vx: 0, vy: 0 });
    tracker.push({ t: 10, x: 50, y: 50 });
    expect(tracker.velocity(10)).toEqual({ vx: 0, vy: 0 });
  });

  it('forgets everything on reset', () => {
    const tracker = createVelocityTracker();
    tracker.push({ t: 0, x: 0, y: 0 });
    tracker.push({ t: 50, x: 0, y: 100 });
    tracker.reset();
    tracker.push({ t: 60, x: 0, y: 100 });
    expect(tracker.velocity(60)).toEqual({ vx: 0, vy: 0 });
  });
});

describe('flingStep', () => {
  it('covers the geometric sum of a decaying speed and keeps the decayed speed', () => {
    const { distance, velocity } = flingStep(2, 16, 0.5);
    // 2 * (1 - 0.5^16) / 0.5
    expect(distance).toBeCloseTo(4 - 4 * Math.pow(0.5, 16), 10);
    expect(velocity).toBeCloseTo(2 * Math.pow(0.5, 16), 10);
  });

  it('with the default decay sums to velocity / (1 - decay) over a long fling', () => {
    let v = 2;
    let total = 0;
    for (let i = 0; i < 1000; i++) {
      const step = flingStep(v, 16);
      total += step.distance;
      v = step.velocity;
    }
    expect(total).toBeCloseTo(2 / (1 - FLING_DECAY_PER_MS), 3);
  });

  it('a zero-length frame moves nothing and keeps the speed', () => {
    expect(flingStep(1.5, 0)).toEqual({ distance: 0, velocity: 1.5 });
  });
});

describe('isFlingVelocity', () => {
  it('is the speed in either direction against the minimum', () => {
    expect(isFlingVelocity(FLING_MIN_VELOCITY)).toBe(true);
    expect(isFlingVelocity(-FLING_MIN_VELOCITY)).toBe(true);
    expect(isFlingVelocity(FLING_MIN_VELOCITY / 2)).toBe(false);
    expect(isFlingVelocity(0)).toBe(false);
  });
});

/** A frame scheduler driven by hand: `run(now)` fires the pending frame. */
function fakeFrames() {
  let pending: ((now: number) => void) | null = null;
  let nextId = 1;
  const cancelled: number[] = [];
  const scheduler: FrameScheduler = {
    request: (callback) => {
      pending = callback;
      return nextId++;
    },
    cancel: (id) => {
      cancelled.push(id);
      pending = null;
    },
  };
  return {
    scheduler,
    cancelled,
    get hasPending() {
      return pending !== null;
    },
    run(now: number) {
      const callback = pending;
      pending = null;
      callback?.(now);
    },
  };
}

describe('startFling', () => {
  it('does not start below the minimum speed', () => {
    const frames = fakeFrames();
    const apply = vi.fn(() => true);
    expect(startFling(FLING_MIN_VELOCITY / 2, apply, frames.scheduler)).toBeNull();
    expect(frames.hasPending).toBe(false);
    expect(apply).not.toHaveBeenCalled();
  });

  it("moves the content by each frame's decayed distance from the second frame on", () => {
    const frames = fakeFrames();
    const distances: number[] = [];
    const handle = startFling(
      1,
      (distance) => {
        distances.push(distance);
        return true;
      },
      frames.scheduler
    );
    expect(handle).not.toBeNull();
    frames.run(1000);
    expect(distances).toEqual([]);
    frames.run(1016);
    frames.run(1032);
    expect(distances).toHaveLength(2);
    expect(distances[0]).toBeCloseTo(flingStep(1, 16).distance, 10);
    expect(distances[1]).toBeCloseTo(flingStep(flingStep(1, 16).velocity, 16).distance, 10);
    expect(distances[1]).toBeLessThan(distances[0]);
    expect(frames.hasPending).toBe(true);
  });

  it('stops when the content stops moving', () => {
    const frames = fakeFrames();
    const apply = vi.fn(() => false);
    startFling(1, apply, frames.scheduler);
    frames.run(0);
    frames.run(16);
    expect(apply).toHaveBeenCalledTimes(1);
    expect(frames.hasPending).toBe(false);
  });

  it('stops once the speed is spent', () => {
    const frames = fakeFrames();
    const apply = vi.fn(() => true);
    startFling(FLING_MIN_VELOCITY * 1.01, apply, frames.scheduler);
    frames.run(0);
    frames.run(16);
    // 1.01 * 0.998^16 < 1: under the minimum after one frame.
    expect(apply).toHaveBeenCalledTimes(1);
    expect(frames.hasPending).toBe(false);
  });

  it('caps a long gap between frames, so a hidden tab does not jump', () => {
    const frames = fakeFrames();
    const distances: number[] = [];
    startFling(
      1,
      (distance) => {
        distances.push(distance);
        return true;
      },
      frames.scheduler
    );
    frames.run(0);
    frames.run(5000);
    expect(distances[0]).toBeCloseTo(flingStep(1, MAX_FRAME_MS).distance, 10);
  });

  it('cancel drops the pending frame', () => {
    const frames = fakeFrames();
    const apply = vi.fn(() => true);
    const handle = startFling(1, apply, frames.scheduler);
    frames.run(0);
    handle?.cancel();
    expect(frames.cancelled).toHaveLength(1);
    expect(frames.hasPending).toBe(false);
    frames.run(16);
    expect(apply).not.toHaveBeenCalled();
  });
});

describe('velocity from a sparse move stream', () => {
  it('reads the speed over the last moves even when the lift comes later', () => {
    // Moves ~90 ms apart (a driver's stepped swipe): the lift 88 ms after
    // the last move still flings at the speed of the last two moves.
    const tracker = createVelocityTracker();
    tracker.push({ t: 0, x: 0, y: 0 });
    tracker.push({ t: 90, x: 0, y: 50 });
    tracker.push({ t: 180, x: 0, y: 100 });
    expect(tracker.velocity(268).vy).toBeCloseTo(50 / 90, 5);
  });

  it('is still when the lift comes more than the window after the last move', () => {
    const tracker = createVelocityTracker();
    tracker.push({ t: 0, x: 0, y: 0 });
    tracker.push({ t: 16, x: 0, y: 40 });
    expect(tracker.velocity(16 + VELOCITY_WINDOW_MS + 1)).toEqual({ vx: 0, vy: 0 });
  });
});

describe('startFling velocity cap', () => {
  it('starts a jump-fast flick at FLING_MAX_VELOCITY', () => {
    const frames = fakeFrames();
    const moved: number[] = [];
    startFling(300 / 6, (d) => (moved.push(d), true), frames.scheduler);
    frames.run(0);
    frames.run(16);
    const expected = flingStep(FLING_MAX_VELOCITY, 16).distance;
    expect(moved[0]).toBeCloseTo(expected, 5);
  });
});
