import { describe, expect, it } from 'vitest';
import { reopenAfterRevalidate, reopenMode, restoredScrollTop } from './paneReopen';

describe('reopenMode', () => {
  const loaded = { loadedSessionId: 's1', sessionId: 's1', messageCount: 12, error: null };

  it('revalidates when the same session is loaded with rows and no error', () => {
    expect(reopenMode(loaded)).toBe('revalidate');
  });

  it('loads fully for a different session', () => {
    expect(reopenMode({ ...loaded, sessionId: 's2' })).toBe('full');
  });

  it('loads fully when nothing was ever loaded', () => {
    expect(reopenMode({ ...loaded, loadedSessionId: null })).toBe('full');
    expect(reopenMode({ ...loaded, loadedSessionId: undefined })).toBe('full');
  });

  it('loads fully over an empty transcript', () => {
    expect(reopenMode({ ...loaded, messageCount: 0 })).toBe('full');
  });

  it('loads fully after an error such as a missing session', () => {
    expect(reopenMode({ ...loaded, error: 'Session not found' })).toBe('full');
  });
});

describe('reopenAfterRevalidate', () => {
  it('is settled only by an applied delta', () => {
    expect(reopenAfterRevalidate('applied')).toBe('settled');
  });

  it('falls through to the full load for a deleted session, a failed poll or no poll', () => {
    // `poll_session` answers `session: null` for an id the store no longer has;
    // the full load is what shows "Session not found" instead of keeping the
    // stale transcript and starting a poll loop on it.
    expect(reopenAfterRevalidate('missing')).toBe('full');
    expect(reopenAfterRevalidate('failed')).toBe('full');
    expect(reopenAfterRevalidate('skipped')).toBe('full');
  });
});

describe('restoredScrollTop', () => {
  // A box freshly reattached: WebKit reset scrollTop to 0. `anchorTop` is the
  // anchored group's top edge relative to the box's top, as laid out now.
  const box = { scrollTop: 0, scrollHeight: 2000, clientHeight: 600 };
  const anchor = { key: 'm-7', offset: 40 };

  it('goes to the bottom when the pane was following it, whatever the anchor says', () => {
    expect(restoredScrollTop({ scrollTop: 120, atBottom: true, anchor: null }, box, null)).toBe(
      1400
    );
    expect(restoredScrollTop({ scrollTop: 120, atBottom: true, anchor }, box, 400)).toBe(1400);
  });

  it('puts the anchored group back at its offset when it is still rendered', () => {
    // The group now sits 900px below the box top; it belongs 40px below it.
    expect(restoredScrollTop({ scrollTop: 480, atBottom: false, anchor }, box, 900)).toBe(860);
  });

  it('anchors against the current scrollTop, not zero', () => {
    expect(
      restoredScrollTop(
        { scrollTop: 480, atBottom: false, anchor },
        { ...box, scrollTop: 300 },
        600
      )
    ).toBe(860);
  });

  it('survives a box of another width: the anchor wins over the pixel offset', () => {
    // Same group, but in a narrower box it has moved from 480px to 1200px down.
    const narrow = { ...box, scrollHeight: 3000 };
    expect(restoredScrollTop({ scrollTop: 480, atBottom: false, anchor }, narrow, 1200)).toBe(1160);
  });

  it('restores the saved offset when the anchored group is gone', () => {
    expect(restoredScrollTop({ scrollTop: 480, atBottom: false, anchor }, box, null)).toBe(480);
    expect(restoredScrollTop({ scrollTop: 480, atBottom: false, anchor: null }, box, null)).toBe(
      480
    );
  });

  it('clamps an offset the trimmed transcript can no longer reach', () => {
    expect(restoredScrollTop({ scrollTop: 1800, atBottom: false, anchor: null }, box, null)).toBe(
      1400
    );
    expect(restoredScrollTop({ scrollTop: -5, atBottom: false, anchor: null }, box, null)).toBe(0);
    expect(restoredScrollTop({ scrollTop: 480, atBottom: false, anchor }, box, 1800)).toBe(1400);
    expect(restoredScrollTop({ scrollTop: 480, atBottom: false, anchor }, box, -10)).toBe(0);
  });

  it('never goes negative when the transcript fits the viewport', () => {
    expect(
      restoredScrollTop(
        { scrollTop: 50, atBottom: true, anchor: null },
        { ...box, scrollHeight: 300 },
        null
      )
    ).toBe(0);
  });
});
