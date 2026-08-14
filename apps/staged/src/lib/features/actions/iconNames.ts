/**
 * Pure name algebra for action icons: the kebab ↔ Pascal conversions between
 * what we store and what Lucide's icon map is keyed by, plus what the picker's
 * grid shows for a given query.
 *
 * Deliberately component-free so it stays cheap to import and testable —
 * loading actual icons is [`./lucideIcons`]'s job.
 */

/** What the empty-search icon grid offers, roughly ordered by how action-ish it is. */
export const CURATED_ICONS = [
  'play',
  'rocket',
  'globe',
  'terminal',
  'server',
  'database',
  'hammer',
  'wrench',
  'flask-conical',
  'bug',
  'zap',
  'monitor',
  'smartphone',
  'package',
  'cloud',
  'code',
  'book-open',
  'refresh-cw',
  'gauge',
  'layout-dashboard',
  'sparkles',
  'shield-check',
  'eye',
  'palette',
];

/** How many search hits the grid renders before asking for a narrower query. */
export const ICON_SEARCH_LIMIT = 60;

/** `flask-conical` → `FlaskConical`, the export name Lucide's icon map is keyed by. */
export function kebabToPascal(kebab: string): string {
  return kebab
    .split('-')
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join('');
}

/** The most-split reading: every capital and every digit opens a segment. */
function splitEverySegment(pascal: string): string {
  const segments: string[] = [];
  for (const char of pascal) {
    const previous = segments[segments.length - 1];
    // A digit ends a segment as well as opening one, so "2x2" reads as three.
    const opensSegment = /[A-Z0-9]/.test(char) || /\d$/.test(previous ?? '');
    if (previous === undefined || opensSegment) segments.push(char);
    else segments[segments.length - 1] += char;
  }
  return segments.join('-').toLowerCase();
}

/**
 * `FlaskConical` → `flask-conical`, the name we store and lucide.dev shows.
 *
 * Pascal-casing is lossy — it drops the segment boundaries, and single-letter
 * and digit segments make several readings plausible (`arrow-down-a-z` and
 * `grid-2x2` are both real). So instead of one rule, candidate spellings are
 * tried most-split first, each round-tripped through [`kebabToPascal`] — the
 * conversion rendering actually uses — and the first survivor wins. That
 * guarantees whatever comes out of here resolves back to a real icon, and it
 * reproduces lucide.dev's own spelling for all but three of ~1,750 icons
 * (`Clock10` reads as `clock-1-0`, where nothing but a lookup table could tell
 * it apart from `ArrowDown01` → `arrow-down-0-1`).
 */
export function pascalToKebab(pascal: string): string {
  const splitAtCaps = pascal.replace(/([a-z\d])([A-Z])/g, '$1-$2');
  const candidates = [
    splitEverySegment(pascal),
    // Only the first letter→digit boundary splits: "Grid2x2" → "grid-2x2".
    splitAtCaps.replace(/([a-zA-Z])(\d)/, '$1-$2').toLowerCase(),
    splitAtCaps.toLowerCase(),
  ];
  return candidates.find((c) => kebabToPascal(c) === pascal) ?? candidates[candidates.length - 1];
}

/**
 * Icon names matching a search query, capped at [`ICON_SEARCH_LIMIT`]. An empty
 * query gets the curated set — painting 1,600 SVGs at once is what the cap and
 * the curated list exist to avoid.
 */
export function searchIconNames(names: string[], query: string): string[] {
  const trimmed = query.trim().toLowerCase().replace(/\s+/g, '-');
  if (!trimmed) return CURATED_ICONS;
  return names.filter((name) => name.includes(trimmed)).slice(0, ICON_SEARCH_LIMIT);
}
