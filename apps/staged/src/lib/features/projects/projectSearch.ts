/**
 * Client-side text search for the landing-page project grid.
 *
 * Plain module (no runes, no store imports) so the matching rule stays
 * trivially unit-testable. Projects carry no description, so only the display
 * name is searched. Applied after the chip filters in ProjectsList.
 */

import type { Project } from '../../types';
import { projectDisplayName } from '../../shared/utils';

/**
 * Case-insensitive substring match on the display name. Whitespace around
 * the query is ignored; an empty (or whitespace-only) query matches every
 * project and returns the input array unchanged.
 */
export function searchProjects(projects: Project[], query: string): Project[] {
  const q = query.trim().toLowerCase();
  if (!q) return projects;
  return projects.filter((p) => projectDisplayName(p).toLowerCase().includes(q));
}
