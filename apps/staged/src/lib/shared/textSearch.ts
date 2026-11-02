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
