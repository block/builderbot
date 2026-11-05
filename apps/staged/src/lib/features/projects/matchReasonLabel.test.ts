import { describe, expect, it } from 'vitest';
import {
  MATCH_CONTEXT_CHARS,
  MATCH_LEAD_CHARS,
  TOOLTIP_MAX_CHARS,
  clampText,
  labelTerms,
  matchLabelSegments,
} from './matchReasonLabel';

/** The segments as text, highlights in [brackets]. */
function render(label: string, terms: string[]): string {
  return matchLabelSegments(label, terms)
    .map((s) => (s.match ? `[${s.text}]` : s.text))
    .join('');
}

describe('matchLabelSegments', () => {
  it('shows a label whose first highlight comes early from its start', () => {
    expect(render('Fix login redirect', ['login'])).toBe('Fix [login] redirect');
    const atLead = `${'x'.repeat(MATCH_LEAD_CHARS)}login`;
    expect(render(atLead, ['login'])).toBe(`${'x'.repeat(MATCH_LEAD_CHARS)}[login]`);
  });

  it('leaves a label with no highlight alone', () => {
    const long = 'Investigate why the session hangs after the user clicks stop twice';
    expect(render(long, ['zzz'])).toBe(long);
    expect(render(long, [])).toBe(long);
  });

  it('starts a few words before a late first highlight, behind an ellipsis', () => {
    const label = 'Investigate why the session hangs after the user clicks stop twice';
    expect(render(label, ['stop'])).toBe('…the user clicks [stop] twice');
  });

  it('keeps only whole words that start within the context distance', () => {
    const label = `${'word '.repeat(10)}abcdefghijklmnop target`;
    // The 16-letter word starts 17 characters before the match: too far.
    expect(MATCH_CONTEXT_CHARS).toBe(16);
    expect(render(label, ['target'])).toBe('…[target]');
  });

  it('includes the start of the word a match falls inside', () => {
    const label = 'Investigate why the session hangs after the user clicked stopping';
    expect(render(label, ['opping'])).toBe('…user clicked st[opping]');
  });

  it('starts at the match itself inside one very long word', () => {
    const label = `See ${'a'.repeat(60)}needle${'b'.repeat(10)}`;
    expect(render(label, ['needle'])).toBe(`…[needle]${'b'.repeat(10)}`);
  });

  it('keeps every highlight after the window, and the ones before it go', () => {
    const label = 'Login: investigate why the session hangs after the user clicks login';
    // The first highlight is at 0, so nothing is windowed.
    expect(render(label, ['login'])).toBe(
      '[Login]: investigate why the session hangs after the user clicks [login]'
    );
    const late = 'Investigate why the session hangs after a login, then login again';
    expect(render(late, ['login'])).toBe('…hangs after a [login], then [login] again');
  });

  it('windows a PR label by its title like any other', () => {
    const label = 'PR #123 · Refactor the session manager so a stopped turn releases its lock';
    expect(render(label, ['lock'])).toBe('…releases its [lock]');
  });
});

describe('labelTerms', () => {
  it('highlights `pr123` and `pr#123` the way the label writes the number', () => {
    expect(labelTerms(['pr123', 'pr#123', 'pr', '#123', '123', 'login'])).toEqual([
      '123',
      '#123',
      'pr',
      '#123',
      '123',
      'login',
    ]);
  });

  it('leaves a word that merely starts with pr alone', () => {
    expect(labelTerms(['preview', 'pr12a'])).toEqual(['preview', 'pr12a']);
  });

  it('lets the PR prefix of a label light up for `pr 123`', () => {
    expect(render('PR #123 · Fix login', labelTerms(['pr', '123']))).toBe(
      '[PR] #[123] · Fix login'
    );
    expect(render('PR #123 · Fix login', labelTerms(['pr#123']))).toBe('PR [#123] · Fix login');
  });
});

describe('clampText', () => {
  it('leaves short text alone', () => {
    expect(clampText('Fix login')).toBe('Fix login');
    const exact = 'x'.repeat(TOOLTIP_MAX_CHARS);
    expect(clampText(exact)).toBe(exact);
  });

  it('cuts long text to the limit, ending in an ellipsis', () => {
    const clamped = clampText('word '.repeat(2_000));
    expect(clamped.length).toBeLessThanOrEqual(TOOLTIP_MAX_CHARS);
    expect(clamped.endsWith('word…')).toBe(true);
    expect(clampText('abcdefghij', 5)).toBe('abcd…');
  });

  it('does not split a surrogate pair', () => {
    expect(clampText('abc😀def', 5)).toBe('abc…');
  });
});
