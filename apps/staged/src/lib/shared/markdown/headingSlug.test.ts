import { describe, expect, it } from 'vitest';

import { createHeadingSlugger, githubHeadingSlug } from './headingSlug';

describe('githubHeadingSlug', () => {
  // Heading → fragment pairs taken from links in real notes.
  it.each([
    ['How the plan behaves in difficult cases', 'how-the-plan-behaves-in-difficult-cases'],
    ["Silencing this app's own alerts", 'silencing-this-apps-own-alerts'],
    ['Need in both directions (rule 22)', 'need-in-both-directions-rule-22'],
    ['Ordered steps', 'ordered-steps'],
  ])('slugs %j as %j', (heading, slug) => {
    expect(githubHeadingSlug(heading)).toBe(slug);
  });

  it('drops punctuation without putting a hyphen in its place', () => {
    expect(githubHeadingSlug('What’s new? (v2.1)')).toBe('whats-new-v21');
    expect(githubHeadingSlug('Q&A: errors, retries; timeouts!')).toBe('qa-errors-retries-timeouts');
  });

  it('keeps hyphens and underscores', () => {
    expect(githubHeadingSlug('Re-run the pre_push hook')).toBe('re-run-the-pre_push-hook');
  });

  it('turns every space into a hyphen, collapsing none', () => {
    expect(githubHeadingSlug('Foo & Bar')).toBe('foo--bar');
    expect(githubHeadingSlug('Before - after')).toBe('before---after');
  });

  it('keeps accented and non-Latin letters, lowercased where they have case', () => {
    expect(githubHeadingSlug('Über Café')).toBe('über-café');
    expect(githubHeadingSlug('ΑΒΓ Δοκιμή')).toBe('αβγ-δοκιμή');
    expect(githubHeadingSlug('日本語の 見出し')).toBe('日本語の-見出し');
    expect(githubHeadingSlug('Заголовок раздела')).toBe('заголовок-раздела');
  });

  it('drops emoji, leaving the hyphen of the space beside them', () => {
    expect(githubHeadingSlug('🚀 Launch plan')).toBe('-launch-plan');
    expect(githubHeadingSlug('Launch 🚀 plan')).toBe('launch--plan');
    expect(githubHeadingSlug('Done ✅')).toBe('done-');
  });

  it('slugs the text of inline code like any other text', () => {
    // The heading `## Calling `write_note()` twice`, as its rendered text.
    expect(githubHeadingSlug('Calling write_note() twice')).toBe('calling-write_note-twice');
    expect(githubHeadingSlug('The --no-verify flag')).toBe('the---no-verify-flag');
  });

  it('keeps digits', () => {
    expect(githubHeadingSlug('Step 2 of 10')).toBe('step-2-of-10');
  });

  it('ignores whitespace around the text', () => {
    expect(githubHeadingSlug('  Ordered steps \n')).toBe('ordered-steps');
  });

  it('is empty for a heading with nothing sluggable', () => {
    expect(githubHeadingSlug('???')).toBe('');
  });
});

describe('createHeadingSlugger', () => {
  it('numbers repeated headings from the second one', () => {
    const slug = createHeadingSlugger();
    expect(['Notes', 'Notes', 'Notes', 'Other', 'notes'].map(slug)).toEqual([
      'notes',
      'notes-1',
      'notes-2',
      'other',
      'notes-3',
    ]);
  });

  it('skips a numbered slug another heading already holds', () => {
    const slug = createHeadingSlugger();
    expect(['Notes', 'Notes', 'Notes 1', 'Notes'].map(slug)).toEqual([
      'notes',
      'notes-1',
      'notes-1-1',
      'notes-2',
    ]);
  });

  it('skips a numbered slug a heading held first', () => {
    const slug = createHeadingSlugger();
    expect(['Notes 1', 'Notes', 'Notes'].map(slug)).toEqual(['notes-1', 'notes', 'notes-2']);
  });

  it('starts over for each document', () => {
    const first = createHeadingSlugger();
    const second = createHeadingSlugger();
    expect(first('Notes')).toBe('notes');
    expect(first('Notes')).toBe('notes-1');
    expect(second('Notes')).toBe('notes');
  });
});
