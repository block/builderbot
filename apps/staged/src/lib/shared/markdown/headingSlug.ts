/**
 * headingSlug — the fragment GitHub gives a markdown heading.
 *
 * Notes link to their own sections the way a README does:
 * `[dry summer](#how-the-beds-cope-with-a-dry-summer)`. The target
 * is not written anywhere in the note; it is derived from the heading's text
 * by GitHub's rule, so the same rule has to be applied here for those links
 * to resolve.
 *
 * The rule is checked against what GitHub's own renderer gives (the anchors
 * `POST /markdown` returns); headingSlug.test.ts holds the pairs.
 */

/**
 * Everything GitHub drops from a heading: whatever is not a letter, a mark,
 * a digit, connector punctuation (`_`), a join control, a hyphen or a space.
 * This is the complement of Ruby's `\p{Word}` plus `-` and space, the set
 * github-slugger generates its table from. Emoji and ASCII punctuation go;
 * accented and non-Latin letters stay.
 *
 * Marks and join controls stay even where they are all that is left of an
 * emoji: `⚠️` leaves its variation selector (U+FE0F) and `👩‍💻` its joiner
 * (U+200D), so a fragment typed by hand without them misses, as on GitHub.
 */
const DROPPED = /[^\p{Alphabetic}\p{M}\p{Nd}\p{Pc}\p{Join_Control}\- ]/gu;

/**
 * Lowercase as GitHub does, one character at a time. `toLowerCase` alone
 * writes a capital sigma that ends a word as `ς`; GitHub writes `σ` wherever
 * it stands. A fragment is matched to a slug whatever its case, so it is
 * lowercased with this too.
 */
export function lowercase(text: string): string {
  return text.replace(/Σ/g, 'σ').toLowerCase();
}

/**
 * The slug for one heading's text, before de-duplication.
 *
 * Lowercased, dropped characters removed, then each space becomes a hyphen.
 * Nothing is collapsed or trimmed, as on GitHub: `Foo & Bar` is `foo--bar`,
 * and a heading that opens with an emoji keeps the hyphen that followed it
 * (`🚀 Launch` is `-launch`). That holds at the edges too. The markdown
 * parser has already dropped the whitespace around a heading, so a space
 * still there stood beside something that has no text, an image say, and
 * GitHub keeps its hyphen (`![icon](x) Title` is `-title`).
 */
export function githubHeadingSlug(text: string): string {
  return lowercase(text).replace(DROPPED, '').replace(/ /g, '-');
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
