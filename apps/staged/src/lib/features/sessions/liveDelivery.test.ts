import { describe, expect, it } from 'vitest';
import {
  LIVE_DELIVERY_UNKNOWN,
  liveDeliveryKnowledge,
  shouldAskLiveDelivery,
} from './liveDelivery';

describe('liveDelivery', () => {
  it('asks while the session is running and nothing is settled', () => {
    expect(shouldAskLiveDelivery('running', LIVE_DELIVERY_UNKNOWN)).toBe(true);
    expect(shouldAskLiveDelivery('running', { supported: false, settled: false })).toBe(true);
  });

  it('never asks about a session that is not running', () => {
    for (const status of ['queued', 'completed', 'cancelled', 'error', undefined] as const) {
      expect(shouldAskLiveDelivery(status, LIVE_DELIVERY_UNKNOWN)).toBe(false);
    }
  });

  it('stops asking once the backend says yes', () => {
    const knowledge = liveDeliveryKnowledge({ supported: true, retryable: false });
    expect(knowledge).toEqual({ supported: true, settled: true });
    expect(shouldAskLiveDelivery('running', knowledge)).toBe(false);
  });

  it('keeps asking after a retryable no (the agent has not connected yet)', () => {
    const knowledge = liveDeliveryKnowledge({
      supported: false,
      retryable: true,
      reason: "the session's agent has not connected yet",
    });
    expect(knowledge).toEqual({ supported: false, settled: false });
    expect(shouldAskLiveDelivery('running', knowledge)).toBe(true);
  });

  it('stops asking after a non-retryable no (another instance owns the session)', () => {
    const knowledge = liveDeliveryKnowledge({
      supported: false,
      retryable: false,
      reason: 'the session is owned by another Staged instance (pid 42)',
    });
    expect(knowledge).toEqual({ supported: false, settled: true });
    expect(shouldAskLiveDelivery('running', knowledge)).toBe(false);
    // A status change resets to unknown, which asks again.
    expect(shouldAskLiveDelivery('running', LIVE_DELIVERY_UNKNOWN)).toBe(true);
  });
});
