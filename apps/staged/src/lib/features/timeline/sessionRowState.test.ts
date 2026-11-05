import { describe, expect, it } from 'vitest';
import { RESUMABLE_REASONS, type CompletionReason } from '../../types';
import { canResumeRow, noteRowPhase, type SessionRowPhase } from './sessionRowState';

const ALL_REASONS: CompletionReason[] = [
  'turn_complete',
  'held_until_cap',
  'hold_stopped',
  'interrupted',
  'project_session_interrupted',
  'crashed',
  'app_quit',
  'unknown',
];

describe('noteRowPhase', () => {
  it('reads a queued or running session as queued or running, whatever the content', () => {
    expect(noteRowPhase({ sessionId: 's', sessionStatus: 'queued', content: '' })).toBe('queued');
    expect(noteRowPhase({ sessionId: 's', sessionStatus: 'running', content: '' })).toBe('running');
    expect(noteRowPhase({ sessionId: 's', sessionStatus: 'running', content: 'Body' })).toBe(
      'running'
    );
  });

  it('reads a finished session that wrote no note as failed', () => {
    for (const sessionStatus of ['completed', 'error', 'cancelled', null]) {
      expect(noteRowPhase({ sessionId: 's', sessionStatus, content: '' })).toBe('failed');
      expect(noteRowPhase({ sessionId: 's', sessionStatus, content: '  \n' })).toBe('failed');
    }
    expect(noteRowPhase({ sessionId: 's', sessionStatus: 'completed' })).toBe('failed');
  });

  it('reads a written note as done', () => {
    expect(noteRowPhase({ sessionId: 's', sessionStatus: 'completed', content: 'Body' })).toBe(
      'done'
    );
    expect(noteRowPhase({ sessionId: 's', sessionStatus: 'error', content: 'Body' })).toBe('done');
  });

  it('reads a note with no session as done, even when empty', () => {
    expect(noteRowPhase({ sessionId: null, sessionStatus: null, content: '' })).toBe('done');
    expect(noteRowPhase({ sessionStatus: null, content: 'Body' })).toBe('done');
  });
});

describe('canResumeRow', () => {
  const row = (overrides: {
    completionReason?: string | null;
    phase?: SessionRowPhase;
    sessionId?: string | null;
    deleting?: boolean;
  }) => ({ sessionId: 's', completionReason: 'crashed', phase: 'failed' as const, ...overrides });

  it('offers Resume for exactly the resumable completion reasons', () => {
    for (const reason of ALL_REASONS) {
      expect(canResumeRow(row({ completionReason: reason })), reason).toBe(
        RESUMABLE_REASONS.has(reason)
      );
    }
    expect(canResumeRow(row({ completionReason: null }))).toBe(false);
  });

  it('offers Resume on a done row whose background wait was cut short', () => {
    expect(canResumeRow(row({ completionReason: 'hold_stopped', phase: 'done' }))).toBe(true);
    expect(canResumeRow(row({ completionReason: 'held_until_cap', phase: 'done' }))).toBe(true);
    expect(canResumeRow(row({ completionReason: 'turn_complete', phase: 'done' }))).toBe(false);
  });

  it('withholds Resume while the session is queued or running', () => {
    expect(canResumeRow(row({ phase: 'queued' }))).toBe(false);
    expect(canResumeRow(row({ phase: 'running' }))).toBe(false);
  });

  it('withholds Resume while the row is being deleted', () => {
    expect(canResumeRow(row({ deleting: true }))).toBe(false);
  });

  it('withholds Resume from a row with no session', () => {
    expect(canResumeRow(row({ sessionId: null }))).toBe(false);
    expect(canResumeRow(row({ sessionId: undefined }))).toBe(false);
  });
});
