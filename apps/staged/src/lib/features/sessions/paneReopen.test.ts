import { describe, expect, it } from 'vitest';
import { reopenMode, restoredScrollTop } from './paneReopen';

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

describe('restoredScrollTop', () => {
  it('goes to the bottom when the pane was following it', () => {
    expect(restoredScrollTop({ scrollTop: 120, atBottom: true }, 2000, 600)).toBe(1400);
  });

  it('restores the saved offset mid-transcript', () => {
    expect(restoredScrollTop({ scrollTop: 480, atBottom: false }, 2000, 600)).toBe(480);
  });

  it('clamps an offset the trimmed transcript can no longer reach', () => {
    expect(restoredScrollTop({ scrollTop: 1800, atBottom: false }, 2000, 600)).toBe(1400);
    expect(restoredScrollTop({ scrollTop: -5, atBottom: false }, 2000, 600)).toBe(0);
  });

  it('never goes negative when the transcript fits the viewport', () => {
    expect(restoredScrollTop({ scrollTop: 50, atBottom: true }, 300, 600)).toBe(0);
  });
});
