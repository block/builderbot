/**
 * Diff Search Service
 *
 * Provides text search across diff content.
 * Searches only the right/after side (new content) for simplicity.
 */

// =============================================================================
// Types
// =============================================================================

/** Location of a match within a line */
export interface MatchLocation {
  startCol: number;
  endCol: number;
}

/** A search match on the right/after side of a diff line */
export interface SearchMatch {
  /** Index into the line arrays */
  lineIndex: number;
  /** Match location on the left/before side (always undefined - search only looks at right side) */
  left?: MatchLocation;
  /** Match location on the right/after side */
  right?: MatchLocation;
}

// =============================================================================
// Constants
// =============================================================================

/** Maximum number of matches to return (for performance) */
export const MAX_MATCHES = 1000;

// =============================================================================
// Search Logic
// =============================================================================

/**
 * Find all occurrences of a query string within a line.
 * Returns array of match locations (case-insensitive).
 */
function findInString(content: string | undefined, query: string): MatchLocation[] {
  if (!content || !query) return [];

  const matches: MatchLocation[] = [];
  const lowerContent = content.toLowerCase();
  const lowerQuery = query.toLowerCase();

  let pos = 0;
  while (pos < lowerContent.length) {
    const idx = lowerContent.indexOf(lowerQuery, pos);
    if (idx === -1) break;

    matches.push({
      startCol: idx,
      endCol: idx + query.length,
    });
    pos = idx + 1; // Allow overlapping matches
  }

  return matches;
}

/**
 * Find all matches in a diff's content.
 * Searches only the right-hand side (after/new content).
 *
 * @param beforeLines - Lines from the "before" side (unused, kept for compatibility)
 * @param afterLines - Lines from the "after" side of the diff
 * @param query - Search query (case-insensitive)
 * @param scope - 'all' to search all lines, 'changes' to search only changed lines
 * @param changedLineIndices - Set of line indices that are in changed regions (only used when scope='changes')
 * @returns Array of matches (limited to MAX_MATCHES)
 */
export function findMatches(
  beforeLines: string[],
  afterLines: string[],
  query: string,
  scope: 'all' | 'changes' = 'all',
  changedLineIndices?: Set<number>
): SearchMatch[] {
  if (!query) return [];

  const matches: SearchMatch[] = [];

  for (let i = 0; i < afterLines.length; i++) {
    // Stop if we've hit the match limit
    if (matches.length >= MAX_MATCHES) {
      console.log(`[diffSearch] Reached match limit of ${MAX_MATCHES}, stopping search`);
      break;
    }

    // If scope is 'changes', skip lines that aren't in changed regions
    if (scope === 'changes' && changedLineIndices && !changedLineIndices.has(i)) {
      continue;
    }

    const rightContent = afterLines[i];
    const rightMatches = findInString(rightContent, query);

    // Skip lines with no matches
    if (rightMatches.length === 0) continue;

    // Add all matches from this line
    for (const location of rightMatches) {
      if (matches.length >= MAX_MATCHES) break;

      matches.push({
        lineIndex: i,
        left: undefined,
        right: location,
      });
    }
  }

  return matches;
}

/**
 * Get a human-readable description of a match's location.
 */
export function describeMatch(match: SearchMatch, currentIndex: number, total: number): string {
  return `${currentIndex + 1}/${total}`;
}
