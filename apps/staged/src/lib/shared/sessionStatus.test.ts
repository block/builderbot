import { describe, expect, it } from 'vitest';
import { isSessionActive, isSessionInFlight } from './sessionStatus';

describe('isSessionInFlight', () => {
  it('holds a slot while running or disconnected, but not while queued or terminal', () => {
    expect(isSessionInFlight('running')).toBe(true);
    expect(isSessionInFlight('disconnected')).toBe(true);
    for (const status of ['queued', 'completed', 'error', 'cancelled', null, undefined]) {
      expect(isSessionInFlight(status)).toBe(false);
    }
  });
});

describe('isSessionActive', () => {
  it('counts queued, running and disconnected sessions as still in flight', () => {
    for (const status of ['queued', 'running', 'disconnected']) {
      expect(isSessionActive(status)).toBe(true);
    }
  });

  it('does not count terminal or unknown statuses', () => {
    for (const status of ['completed', 'error', 'cancelled', null, undefined]) {
      expect(isSessionActive(status)).toBe(false);
    }
  });
});
