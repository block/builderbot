/**
 * Keeping the objects a refetch left unchanged.
 *
 * Every read of the backend builds new objects. A store that applies a list
 * as fetched hands every reader a new row for a change to one row, or to
 * none, and each of them re-renders. With the unchanged rows' objects kept,
 * only what reads a changed row re-runs, and a list that changed nothing is
 * the list already held, which wakes nothing.
 *
 * Used by the stores that apply whole lists: the project list
 * (projectsData) and the tag snapshot (tagState).
 */

/**
 * Whether two rows carry the same fields with the same values, compared one
 * level deep. Right for flat rows, whose fields are all primitives. A nested
 * value would compare by identity and read as changed, which costs a
 * re-render, never a stale row.
 */
export function sameRow<T extends object>(a: T, b: T): boolean {
  if (a === b) return true;
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  // Both ways round, so a field only one of them carries counts.
  for (const key of Object.keys(left)) {
    if (!Object.is(left[key], right[key])) return false;
  }
  for (const key of Object.keys(right)) {
    if (!Object.is(left[key], right[key])) return false;
  }
  return true;
}

/**
 * `fetched`, with the row `current` holds under the same key in place of
 * each row that equals it (see sameRow). Returns `current` itself when that
 * leaves the same rows in the same order. The result always equals `fetched`
 * value for value, in `fetched`'s order: only object identity differs.
 *
 * `keyOf` must give each row of a list its own key.
 */
export function reuseUnchangedRows<T extends object>(
  current: T[],
  fetched: readonly T[],
  keyOf: (row: T) => string
): T[] {
  const shown = new Map(current.map((row) => [keyOf(row), row]));
  let unchanged = current.length === fetched.length;
  const next = fetched.map((row, index) => {
    const existing = shown.get(keyOf(row));
    const kept = existing && sameRow(existing, row) ? existing : row;
    if (kept !== current[index]) unchanged = false;
    return kept;
  });
  return unchanged ? current : next;
}
