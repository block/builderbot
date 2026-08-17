import { describe, expect, it } from 'vitest';
import type { ActiveSessionInfo } from '../../types';
import { quitPromptDescription, quitSessionLabel } from './quitPromptCopy';

function session(overrides: Partial<ActiveSessionInfo> = {}): ActiveSessionInfo {
  return {
    sessionId: 's1',
    projectId: 'p1',
    branchId: 'b1',
    sessionType: 'review',
    status: 'running',
    ...overrides,
  };
}

describe('quitSessionLabel', () => {
  it('names the session type and where it runs', () => {
    expect(quitSessionLabel(session(), 'fix-login')).toBe('review on fix-login');
  });

  it('marks queued sessions', () => {
    expect(quitSessionLabel(session({ status: 'queued' }), 'fix-login')).toBe(
      'review on fix-login (queued)'
    );
  });

  it('falls back to "session" for an unknown or missing type', () => {
    expect(quitSessionLabel(session({ sessionType: null }), 'fix-login')).toBe(
      'session on fix-login'
    );
    expect(quitSessionLabel(session({ sessionType: 'mystery' }), 'fix-login')).toBe(
      'session on fix-login'
    );
  });

  it('drops the location when there is none to show', () => {
    expect(quitSessionLabel(session({ sessionType: 'note' }), null)).toBe('note');
  });
});

describe('quitPromptDescription', () => {
  it('reads singular for one session', () => {
    expect(quitPromptDescription(['commit on fix-login'], 0)).toBe(
      '1 session is still running. Quitting will stop it. commit on fix-login.'
    );
  });

  it('lists every session for a plural count', () => {
    expect(quitPromptDescription(['commit on fix-login', 'note on docs'], 0)).toBe(
      '2 sessions are still running. Quitting will stop them. commit on fix-login, note on docs.'
    );
  });

  it('mentions running actions only when there are some', () => {
    expect(quitPromptDescription(['commit on fix-login'], 1)).toContain(
      '1 running action will also stop.'
    );
    expect(quitPromptDescription(['commit on fix-login'], 3)).toContain(
      '3 running actions will also stop.'
    );
    expect(quitPromptDescription(['commit on fix-login'], 0)).not.toContain('action');
  });
});
