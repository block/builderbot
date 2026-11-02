import { describe, expect, it } from 'vitest';
import type { ChildSessionInfo } from '../../types';
import {
  buildChildSessionBadges,
  CHILD_BADGE_TITLE_MAX_CHARS,
  runningChildSessionIds,
  runningSecondaryMeta,
} from './childSessionBadges';

function child(overrides: Partial<ChildSessionInfo> = {}): ChildSessionInfo {
  return {
    sessionId: 'session-1',
    sessionType: 'commit',
    status: 'running',
    title: 'Fix the login flow',
    branchId: 'branch-1',
    ...overrides,
  };
}

describe('buildChildSessionBadges', () => {
  it('keys each badge by its session and carries kind, status and title', () => {
    const badges = buildChildSessionBadges([
      child({ sessionId: 'a', sessionType: 'commit', status: 'running' }),
      child({ sessionId: 'b', sessionType: 'note', status: 'queued', title: 'Research auth' }),
    ]);

    expect(badges).toEqual([
      {
        key: 'a',
        sessionType: 'commit',
        status: 'running',
        label: 'Fix the login flow',
        title: 'Fix the login flow',
      },
      {
        key: 'b',
        sessionType: 'note',
        status: 'queued',
        label: 'Research auth',
        title: 'Research auth',
      },
    ]);
  });

  it('lets a live ACP title replace a commit title, as the branch row does', () => {
    const [badge] = buildChildSessionBadges([child({ title: 'make the change' })], {
      'session-1': 'Refactor the session store',
    });

    expect(badge.title).toBe('Refactor the session store');
  });

  it('never applies a live title to a note', () => {
    const [badge] = buildChildSessionBadges(
      [child({ sessionType: 'note', title: 'investigate the queue' })],
      { 'session-1': 'Live title' }
    );

    expect(badge.title).toBe('investigate the queue');
  });

  it('falls back to the branch row placeholder for an untitled review', () => {
    const [badge] = buildChildSessionBadges([child({ sessionType: 'review', title: null })]);

    expect(badge.title).toBe('Code Review');
    expect(badge.label).toBe('Code Review');
  });

  it('strips embedded context blocks from a prompt-derived commit title', () => {
    const [badge] = buildChildSessionBadges([
      child({ title: 'Fix the login flow <launch-context>repo: block/staged</launch-context>' }),
    ]);

    expect(badge.title).toBe('Fix the login flow');
  });

  it('clips a long label with an ellipsis but keeps the whole title', () => {
    const title = 'Investigate why the branch queue starves reviews behind long commits';
    const [badge] = buildChildSessionBadges([child({ title })]);

    expect(badge.title).toBe(title);
    expect(Array.from(badge.label).length).toBeLessThanOrEqual(CHILD_BADGE_TITLE_MAX_CHARS);
    expect(badge.label.endsWith('…')).toBe(true);
    expect(title.startsWith(badge.label.slice(0, -1))).toBe(true);
  });

  it('collapses a multi-line prompt into one line', () => {
    const [badge] = buildChildSessionBadges([child({ title: 'Fix the\n\n  login flow' })]);

    expect(badge.label).toBe('Fix the login flow');
  });

  it('gives two children in the same repo their own badges', () => {
    const badges = buildChildSessionBadges([
      child({ sessionId: 'a' }),
      child({ sessionId: 'b', sessionType: 'note' }),
    ]);

    expect(badges.map((badge) => badge.key)).toEqual(['a', 'b']);
    expect(badges.map((badge) => badge.sessionType)).toEqual(['commit', 'note']);
  });
});

describe('runningChildSessionIds', () => {
  it('collects running children across parents and skips queued ones', () => {
    expect(
      runningChildSessionIds({
        'parent-1': [child({ sessionId: 'a' }), child({ sessionId: 'b', status: 'queued' })],
        'parent-2': [child({ sessionId: 'c' })],
      })
    ).toEqual(['a', 'c']);
  });
});

describe('runningSecondaryMeta', () => {
  it('shows the live hint when nothing is in flight', () => {
    expect(runningSecondaryMeta([], 'Reading the migration')).toBe('Reading the migration');
  });

  it('falls back to a placeholder before the first hint arrives', () => {
    expect(runningSecondaryMeta([], undefined)).toBe('Generating note');
  });

  it('yields the meta line to child badges', () => {
    const badges = buildChildSessionBadges([child()]);
    expect(runningSecondaryMeta(badges, 'Reading the migration')).toBeUndefined();
  });
});
