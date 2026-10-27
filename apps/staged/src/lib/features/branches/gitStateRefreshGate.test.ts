import { describe, expect, it } from 'vitest';
import { GIT_STATE_FETCH_TTL_MS, shouldRefreshGitStateOnActivation } from './gitStateRefreshGate';

describe('shouldRefreshGitStateOnActivation', () => {
  const now = 1_000_000;

  it('refreshes a card that has never refreshed', () => {
    expect(shouldRefreshGitStateOnActivation(null, now)).toBe(true);
  });

  it('skips when the last refresh is inside the fetch TTL', () => {
    expect(shouldRefreshGitStateOnActivation(now - 1, now)).toBe(false);
    expect(shouldRefreshGitStateOnActivation(now - GIT_STATE_FETCH_TTL_MS + 1, now)).toBe(false);
  });

  it('refreshes once the last refresh is at or past the fetch TTL', () => {
    expect(shouldRefreshGitStateOnActivation(now - GIT_STATE_FETCH_TTL_MS, now)).toBe(true);
    expect(shouldRefreshGitStateOnActivation(now - GIT_STATE_FETCH_TTL_MS * 10, now)).toBe(true);
  });

  it('does not treat a clock that ran backwards as stale', () => {
    expect(shouldRefreshGitStateOnActivation(now + 5_000, now)).toBe(false);
  });

  it('honours an explicit TTL', () => {
    expect(shouldRefreshGitStateOnActivation(now - 500, now, 1_000)).toBe(false);
    expect(shouldRefreshGitStateOnActivation(now - 1_000, now, 1_000)).toBe(true);
  });
});
