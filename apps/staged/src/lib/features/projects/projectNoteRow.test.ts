import { describe, expect, it } from 'vitest';
import type { ChildSessionBadgeProps } from '../timeline/childSessionBadges';
import { projectNoteRow, type ProjectNoteRowSource } from './projectNoteRow';

const NOW = 10 * 60_000;

function note(overrides: Partial<ProjectNoteRowSource> = {}): ProjectNoteRowSource {
  return {
    sessionId: 'session-1',
    sessionStatus: 'completed',
    content: 'Body',
    title: 'A finished note',
    completionReason: null,
    createdAt: 0,
    updatedAt: 5 * 60_000,
    completedAt: 5 * 60_000,
    ...overrides,
  };
}

const badge: ChildSessionBadgeProps = {
  key: 'child-1',
  sessionType: 'commit',
  status: 'running',
  label: 'Fix login',
  title: 'Fix login',
};

describe('projectNoteRow', () => {
  it('reads a written note as a note titled by it, dated by when it was written', () => {
    expect(projectNoteRow(note(), { deleting: false, nowMs: NOW })).toEqual({
      type: 'note',
      title: 'A finished note',
      secondaryMeta: '5m ago',
    });
    expect(projectNoteRow(note({ title: '' }), { deleting: false, nowMs: NOW }).title).toBe(
      'Untitled note'
    );
    expect(
      projectNoteRow(note({ updatedAt: 8 * 60_000 }), { deleting: false, nowMs: NOW }).secondaryMeta
    ).toBe('Updated 2m ago');
  });

  it('reads a running note as generating, titled by its live title and subtitled by its hint', () => {
    const running = note({ sessionStatus: 'running', content: '', title: '' });
    expect(projectNoteRow(running, { deleting: false, nowMs: NOW })).toEqual({
      type: 'generating-note',
      title: 'Generating note…',
      secondaryMeta: 'Generating note',
    });
    expect(
      projectNoteRow(running, {
        deleting: false,
        nowMs: NOW,
        liveTitle: 'Plan the rollout',
        liveHint: 'Reading the deploy docs',
      })
    ).toEqual({
      type: 'generating-note',
      title: 'Plan the rollout',
      secondaryMeta: 'Reading the deploy docs',
    });
  });

  it('lets child badges take a running row’s subtitle over', () => {
    const running = note({ sessionStatus: 'running', content: '' });
    expect(
      projectNoteRow(running, {
        deleting: false,
        nowMs: NOW,
        liveHint: 'Reading the deploy docs',
        childBadges: [badge],
      }).secondaryMeta
    ).toBeUndefined();
  });

  it('reads a queued note the way it reads a running one', () => {
    const queued = note({ sessionStatus: 'queued', content: '', title: '' });
    expect(projectNoteRow(queued, { deleting: false, nowMs: NOW, liveHint: 'Starting' })).toEqual({
      type: 'generating-note',
      title: 'Generating note…',
      secondaryMeta: 'Starting',
    });
  });

  it('titles a failed note with its reason-specific failure copy and no subtitle', () => {
    const failed = (reason: string | null) =>
      projectNoteRow(
        note({ sessionStatus: 'error', content: '', title: '', completionReason: reason }),
        { deleting: false, nowMs: NOW }
      );
    expect(failed('crashed')).toEqual({
      type: 'failed-note',
      title: 'Session crashed — no note created',
      secondaryMeta: undefined,
    });
    expect(failed('interrupted').title).toBe('Session stopped — no note created');
    expect(failed(null).title).toBe('Session finished — no note created');
  });

  it('subtitles a deleting row "Deleting..." whatever its phase', () => {
    const live = { deleting: true, nowMs: NOW, liveHint: 'Reading' };
    expect(projectNoteRow(note(), live).secondaryMeta).toBe('Deleting...');
    expect(
      projectNoteRow(note({ sessionStatus: 'running', content: '' }), live).secondaryMeta
    ).toBe('Deleting...');
    expect(
      projectNoteRow(
        note({ sessionStatus: 'error', content: '', completionReason: 'crashed' }),
        live
      ).secondaryMeta
    ).toBe('Deleting...');
  });
});
