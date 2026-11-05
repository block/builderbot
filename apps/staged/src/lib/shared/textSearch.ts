/**
 * Multi-word matching shared by the client-side search fields: the landing
 * grid and sidebar, All repos, the branch/note project pickers, the hashtag
 * and branch dropdowns, and the GitHub repo picker.
 *
 * A query is split into whitespace-separated terms, and a candidate matches
 * when every term is a case-insensitive substring of at least one of its
 * searchable fields — in any order, not necessarily adjacent, and not
 * necessarily all in the same field. So "one three" finds a project named
 * "one two three", "three one" finds it too, and "one four" does not. A
 * single-term query is the plain substring match it always was.
 */

/** Lower-cased, whitespace-separated terms; empty for a blank query. */
export function queryTerms(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter(Boolean);
}

/**
 * Whether every term appears in at least one of `fields` (null/undefined and
 * empty fields are skipped). No terms — a blank query — matches everything.
 */
export function matchesAllTerms(
  terms: readonly string[],
  ...fields: Array<string | null | undefined>
): boolean {
  if (terms.length === 0) return true;
  const haystacks = fields.filter((f): f is string => !!f).map((f) => f.toLowerCase());
  return terms.every((term) => haystacks.some((h) => h.includes(term)));
}

/**
 * The PR number a term reads as — `123`, `#123`, `pr123` or `pr#123` (terms
 * are lower-cased) — or null. Callers match such a term against PR numbers
 * exactly (so `12` doesn't find PR 123) on top of the usual substring match
 * against text.
 */
export function termPrNumber(term: string): number | null {
  const digits = /^(?:pr)?#?(\d+)$/.exec(term)?.[1];
  return digits === undefined ? null : Number(digits);
}

export interface HighlightSegment {
  text: string;
  match: boolean;
}

/**
 * `text` split into the runs that do and don't contain one of `terms`
 * (lower-cased, as queryTerms returns them), so a result can render its
 * highlights as `<mark>` elements rather than through `{@html}`. Overlapping
 * and adjacent matches merge into one run. No terms, or a text whose
 * lower-cased form changes length (some non-ASCII letters, which would shift
 * every offset after them), yields the whole text as one unmatched run.
 */
export function highlightSegments(text: string, terms: readonly string[]): HighlightSegment[] {
  if (!text) return [];
  const lower = text.toLowerCase();
  if (terms.length === 0 || lower.length !== text.length) return [{ text, match: false }];
  const marked = new Uint8Array(text.length);
  for (const term of terms) {
    if (!term) continue;
    for (let i = lower.indexOf(term); i !== -1; i = lower.indexOf(term, i + 1)) {
      marked.fill(1, i, i + term.length);
    }
  }
  const segments: HighlightSegment[] = [];
  let start = 0;
  for (let i = 1; i <= text.length; i++) {
    if (i === text.length || marked[i] !== marked[start]) {
      segments.push({ text: text.slice(start, i), match: marked[start] === 1 });
      start = i;
    }
  }
  return segments;
}
