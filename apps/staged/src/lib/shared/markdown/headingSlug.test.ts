import { describe, expect, it } from 'vitest';

import { createHeadingSlugger, githubHeadingSlug } from './headingSlug';

describe('githubHeadingSlug', () => {
  // Plain headings: several words, a dropped apostrophe, parentheses and digits.
  it.each([
    ['How the beds cope with a dry summer', 'how-the-beds-cope-with-a-dry-summer'],
    ["Thinning the gardener's own seedlings", 'thinning-the-gardeners-own-seedlings'],
    ['Water on both sides (bed 12)', 'water-on-both-sides-bed-12'],
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

  it('keeps combining marks, which are not letters', () => {
    // The virama (U+094D) in स्ते and the acute of a decomposed é (U+0301)
    // are marks: dropped, the slugs would be नमसते-दुनिया and ecole.
    expect(githubHeadingSlug('नमस्ते दुनिया')).toBe('नमस्ते-दुनिया');
    expect(githubHeadingSlug('e\u0301cole')).toBe('e\u0301cole');
    expect(githubHeadingSlug('x\u030c e\u030a')).toBe('x\u030c-e\u030a');
  });

  it('keeps the variation selector and joiner an emoji leaves behind, as GitHub does', () => {
    // `⚠️` is U+26A0 U+FE0F. The sign goes and the selector, a mark, stays,
    // so the fragment is not `-kiwi`. Not tolerated on purpose: the slug is
    // GitHub's, and a link copied from there carries the selector too.
    expect(githubHeadingSlug('\u26a0\ufe0f Kiwi')).toBe('\ufe0f-kiwi');
    expect(githubHeadingSlug('Kiwi \u26a0\ufe0f plum')).toBe('kiwi-\ufe0f-plum');
    expect(githubHeadingSlug('\u26a0 Fig')).toBe('-fig');
    // 👩‍💻 is two emoji around a zero-width joiner; 1️⃣ is a digit and two marks.
    expect(githubHeadingSlug('\u{1f469}\u200d\u{1f4bb} Olive')).toBe('\u200d-olive');
    expect(githubHeadingSlug('1\ufe0f\u20e3 Pear')).toBe('1\ufe0f\u20e3-pear');
    expect(githubHeadingSlug('Zero\u200bwidth\u200cjoiner')).toBe('zerowidth\u200cjoiner');
  });

  it('slugs an emoji shortcode by its name: it is not turned into an emoji here', () => {
    // GitHub draws `:rocket:` as 🚀 yet slugs the name it was written with.
    expect(githubHeadingSlug(':rocket: Mango')).toBe('rocket-mango');
    expect(githubHeadingSlug('Papaya :rocket: guava')).toBe('papaya-rocket-guava');
  });

  it('lowercases a capital sigma to σ even at the end of a word', () => {
    expect(githubHeadingSlug('ΌΣΟΣ')).toBe('όσοσ');
    expect(githubHeadingSlug('ΑΣ ΑΣ. ΑΣΑ')).toBe('ασ-ασ-ασα');
    // A final sigma the author wrote stays as written.
    expect(githubHeadingSlug('Σίσυφος')).toBe('σίσυφος');
  });

  // Made-up headings, with the anchors GitHub's renderer (`POST /markdown`)
  // gave for them on 2026-11-23. The left side is the heading's text as
  // rendered, which is what is slugged here.
  it.each([
    ['Quince & Lemon', 'quince--lemon'],
    ['Melon   peach', 'melon---peach'],
    ['Grape #commit:0a1b2c3d4e5f', 'grape-commit0a1b2c3d4e5f'],
    ['Grape #note:abc-123 tail', 'grape-noteabc-123-tail'],
    ['Snake_case dunder _esc_', 'snake_case-dunder-_esc_'],
    ['(Optional) nectarine.', 'optional-nectarine'],
    ['Plum — damson – sloe', 'plum--damson--sloe'],
    ['Über Straße İstanbul', 'über-straße-i\u0307stanbul'],
    ['ǅemal ǈ ǋ', 'ǆemal-ǉ-ǌ'],
    ['ẞ ß ſ K Å', 'ß-ß-ſ-k-å'],
    ['café ½ ² ① Ⅷ', 'café----ⅷ'],
    ['Ⓐ ⓐ ① ㊀ ℃', 'ⓐ-ⓐ---'],
    ['･ｶﾞ ー 々 〆', 'ｶﾞ-ー-々-〆'],
    ['٣ ௩ ৩ ๓', '٣-௩-৩-๓'],
    ['· ‧ · ⁀ ⁔ ︳', '---⁀-⁔-︳'],
    ['a‿b ＿c', 'a‿b-＿c'],
    ['$ + < = > ^ ` | ~', '--------'],
    ['™ © ® ℠ № ℗', '-----'],
    ['_ - _', '_---_'],
    ['Tab‑hyphen non‑breaking space', 'tabhyphen-nonbreaking-space'],
    ['🇦🇺 Flag 👍🏽 tone', '-flag--tone'],
    ['Emoji only 🚀', 'emoji-only-'],
  ])('slugs %j as GitHub does: %j', (heading, slug) => {
    expect(githubHeadingSlug(heading)).toBe(slug);
  });

  it('slugs the text of inline code like any other text', () => {
    // The heading `## Calling `write_note()` twice`, as its rendered text.
    expect(githubHeadingSlug('Calling write_note() twice')).toBe('calling-write_note-twice');
    expect(githubHeadingSlug('The --no-verify flag')).toBe('the---no-verify-flag');
  });

  it('keeps digits', () => {
    expect(githubHeadingSlug('Step 2 of 10')).toBe('step-2-of-10');
  });

  it('keeps the hyphen of a space at either edge', () => {
    // The parser trims a heading, so a space still at an edge stood beside
    // something with no text: `## ![icon](x) Title`, `## Title ![icon](x)`,
    // or the padding inside a code span, as in ``## `  hazel  ` nut``.
    expect(githubHeadingSlug(' Heron')).toBe('-heron');
    expect(githubHeadingSlug('Ibis ')).toBe('ibis-');
    expect(githubHeadingSlug(' hazel  nut')).toBe('-hazel--nut');
  });

  it('drops a line break, a tab and a no-break space without a hyphen', () => {
    expect(githubHeadingSlug('Walnut\npecan')).toBe('walnutpecan');
    expect(githubHeadingSlug('Cherry\tapricot')).toBe('cherryapricot');
    expect(githubHeadingSlug('macadamia\u00a0nut')).toBe('macadamianut');
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

  it('numbers headings with nothing sluggable, and a lone hyphen, as GitHub does', () => {
    // `???` twice: GitHub gives the first no anchor at all and the second `-1`.
    expect(['???', '???'].map(createHeadingSlugger())).toEqual(['', '-1']);
    expect(['-', '-'].map(createHeadingSlugger())).toEqual(['-', '--1']);
  });

  it('starts over for each document', () => {
    const first = createHeadingSlugger();
    const second = createHeadingSlugger();
    expect(first('Notes')).toBe('notes');
    expect(first('Notes')).toBe('notes-1');
    expect(second('Notes')).toBe('notes');
  });
});
