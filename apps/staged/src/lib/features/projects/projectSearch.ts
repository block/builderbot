/**
 * Client-side text search for the landing-page project grid.
 *
 * Plain module (no runes, no store imports) so the matching rule stays
 * trivially unit-testable. Projects carry no description, so only the display
 * name is searched. Applied after the chip filters in ProjectsList.
 *
 * `visibleProjects` is the whole composition the grid shows — chip filters,
 * sticky return-target card, then the query — lifted out of the component so
 * the wiring between the three is testable without a Svelte runtime. The grid
 * renders `withStickyProject` and hides the cards the query rules out, so
 * `visibleProjects` is the cards left on screen, not the ones mounted.
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

/**
 * The chip-filter result with the sticky return-target card re-inserted: what
 * the grid shows before the search narrows it.
 *
 * `chipFiltered` is the chip-filter result. When `stickyId` names a project the
 * chips excluded, it is re-inserted — rebuilt from the full `projects` list
 * rather than appended so the card keeps its place in the grid, the position
 * the scroll restore captured. With no re-insertion, `chipFiltered` is
 * returned unchanged.
 */
export function withStickyProject(
  projects: Project[],
  chipFiltered: Project[],
  stickyId: string | null
): Project[] {
  if (!stickyId || chipFiltered.some((p) => p.id === stickyId)) return chipFiltered;
  const matched = new Set(chipFiltered.map((p) => p.id));
  return projects.filter((p) => matched.has(p.id) || p.id === stickyId);
}

/**
 * The projects the landing-page grid shows, in grid order.
 *
 * The query is applied to whatever the chips (and the sticky exception) left.
 * The sticky card is not exempt from the search: the exception exists so a
 * card the user just visited doesn't vanish because visiting it changed its
 * read state, whereas a typed query is a deliberate narrowing, so a sticky
 * card that doesn't match it is hidden.
 *
 * With no sticky re-insertion and an empty query, `chipFiltered` is returned
 * unchanged.
 */
export function visibleProjects(
  projects: Project[],
  chipFiltered: Project[],
  stickyId: string | null,
  query: string
): Project[] {
  return searchProjects(withStickyProject(projects, chipFiltered, stickyId), query);
}

/**
 * The ⌘1–9 number of each card on screen, by project id: the first nine of
 * `shown` (visibleProjects' result) in grid order. The grid keeps cards the
 * query hides mounted, so a card's place in the rendered list is not its
 * place on screen; ⌘N opens `shown[N - 1]`, and these are the numbers the
 * overlay has to show for that.
 */
export function projectShortcutNumbers(shown: Project[]): Map<string, number> {
  return new Map(shown.slice(0, 9).map((p, i) => [p.id, i + 1]));
}
