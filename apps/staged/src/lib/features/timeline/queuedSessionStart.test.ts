import { describe, expect, it } from 'vitest';
import { canStartQueuedSessions } from './queuedSessionStart';

describe('canStartQueuedSessions', () => {
  it('offers Start on an idle branch', () => {
    expect(canStartQueuedSessions({ hasActiveSession: false, gitActionRunning: false })).toBe(true);
  });

  it('withholds Start while a git action holds the branch', () => {
    expect(canStartQueuedSessions({ hasActiveSession: false, gitActionRunning: true })).toBe(false);
  });

  it('withholds Start while another session is running', () => {
    expect(canStartQueuedSessions({ hasActiveSession: true, gitActionRunning: false })).toBe(false);
  });
});
