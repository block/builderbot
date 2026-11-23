/**
 * headingSlug — the fragment GitHub gives a markdown heading.
 *
 * Notes link to their own sections the way a README does:
 * `[difficult cases](#how-the-plan-behaves-in-difficult-cases)`. The target
 * is not written anywhere in the note; it is derived from the heading's text
 * by GitHub's rule, so the same rule has to be applied here for those links
 * to resolve.
 */

/**
 * Everything GitHub drops from a heading: whatever is not a letter, a mark,
 * a digit, connector punctuation (`_`), a hyphen or a space. This is the
 * complement of Ruby's `\p{Word}` plus `-` and space, the set github-slugger
 * generates its table from. Emoji and ASCII punctuation go; accented and
 * non-Latin letters stay.
 */
const DROPPED = /[^\p{Alphabetic}\p{M}\p{Nd}\p{Pc}\- ]/gu;

/**
 * The slug for one heading's text, before de-duplication.
 *
 * Lowercased, dropped characters removed, then each space becomes a hyphen.
 * Nothing is collapsed or trimmed afterwards, as on GitHub: `Foo & Bar` is
 * `foo--bar`, and a heading that opens with an emoji keeps the hyphen that
 * followed it (`🚀 Launch` is `-launch`). Only the whitespace around the
 * text goes, which the markdown parser has already dropped from a heading.
 */
export function githubHeadingSlug(text: string): string {
  return text.trim().toLowerCase().replace(DROPPED, '').replace(/ /g, '-');
}

/**
 * Slugs for a document's headings, in order, de-duplicated as GitHub does:
 * a repeated slug gets `-1`, `-2`, … appended. A suffixed slug that some
 * other heading already holds is skipped (after `Notes`, `Notes`, a heading
 * `Notes 1` is `notes-1-1`), so every slug in one document is distinct.
 *
 * One slugger covers one document; make a new one for each.
 */
export function createHeadingSlugger(): (text: string) => string {
  const used = new Map<string, number>();
  return (text) => {
    const base = githubHeadingSlug(text);
    let slug = base;
    let count = used.get(base) ?? 0;
    while (used.has(slug)) {
      count += 1;
      slug = `${base}-${count}`;
    }
    used.set(base, count);
    if (slug !== base) used.set(slug, 0);
    return slug;
  };
}
