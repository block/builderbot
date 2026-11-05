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
 * Where each code unit of `lower` (`text.toLowerCase()`) came from in
 * `text`: the start and end of the code point that produced it. Lower-casing
 * maps code point by code point (the one context rule, Greek final sigma,
 * keeps the length), so walking `text` and adding up each code point's
 * lower-cased length lines the two up. Null if they somehow don't.
 */
function lowerCaseOrigins(
  text: string,
  lower: string
): { start: Uint32Array; end: Uint32Array } | null {
  const start = new Uint32Array(lower.length);
  const end = new Uint32Array(lower.length);
  let at = 0;
  let from = 0;
  for (const ch of text) {
    const width = ch.toLowerCase().length;
    if (at + width > lower.length) return null;
    start.fill(from, at, at + width);
    end.fill(from + ch.length, at, at + width);
    at += width;
    from += ch.length;
  }
  return at === lower.length ? { start, end } : null;
}

/**
 * `text` split into the runs that do and don't contain one of `terms`
 * (lower-cased, as queryTerms returns them), so a result can render its
 * highlights as `<mark>` elements rather than through `{@html}`. Overlapping
 * and adjacent matches merge into one run. No terms yields the whole text as
 * one unmatched run.
 *
 * Matches are found in the lower-cased text, as matchesAllTerms finds them.
 * When lower-casing changes the length (`İ` becomes `i̇`, two code units),
 * offsets past that point no longer line up with `text`, so each match is
 * mapped back through the code points it covers; one that covers part of a
 * code point's lower-cased form highlights the whole code point.
 */
export function highlightSegments(text: string, terms: readonly string[]): HighlightSegment[] {
  if (!text) return [];
  if (terms.length === 0) return [{ text, match: false }];
  const lower = text.toLowerCase();
  const origins = lower.length === text.length ? null : lowerCaseOrigins(text, lower);
  if (lower.length !== text.length && !origins) return [{ text, match: false }];
  const marked = new Uint8Array(text.length);
  for (const term of terms) {
    if (!term) continue;
    for (let i = lower.indexOf(term); i !== -1; i = lower.indexOf(term, i + 1)) {
      const end = i + term.length;
      if (origins) marked.fill(1, origins.start[i], origins.end[end - 1]);
      else marked.fill(1, i, end);
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
