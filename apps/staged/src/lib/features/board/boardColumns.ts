/**
 * A board's columns: "No ‹group›" first, then one per tag in the group's
 * order, each with the projects whose tag in the group it is. Hidden columns
 * are left out (with their cards) and listed separately so the board header
 * can bring them back; the layout also counts the showing cards they hold,
 * so a board with nothing on it can say where its matches went.
 *
 * Cards keep the order the projects come in, which is the grid's: newest
 * first. Every card the filter chips allow is in its column, search matches
 * or not, so a keystroke hides cards rather than rebuilding them; a column's
 * count is the cards a search leaves showing.
 */

import type { Project, Tag, TagGroup } from '../../types';
import { NO_TAG_COLUMN_KEY, type ResolvedColumnPrefs } from './boardPrefs';

export interface BoardColumn {
  /** A tag id, or NO_TAG_COLUMN_KEY. */
  key: string;
  /** Null for "No ‹group›". */
  tag: Tag | null;
  name: string;
  /** The column's cards, search misses included, newest first. */
  projects: Project[];
  /** The cards a search leaves showing. */
  count: number;
  collapsed: boolean;
}

export interface HiddenBoardColumn {
  key: string;
  tag: Tag | null;
  name: string;
}

export interface BoardLayout {
  columns: BoardColumn[];
  hidden: HiddenBoardColumn[];
  /** The cards a search leaves showing that sit in hidden columns. */
  hiddenCount: number;
}

/** The name of the column for projects without a tag in `group`. */
export function noTagColumnName(group: Pick<TagGroup, 'name'>): string {
  return `No ${group.name}`;
}

/** Every column the board could show, in order, before hiding any. */
function columnHeads(group: TagGroup, tags: readonly Tag[]): HiddenBoardColumn[] {
  return [
    { key: NO_TAG_COLUMN_KEY, tag: null, name: noTagColumnName(group) },
    ...tags.map((tag) => ({ key: tag.id, tag, name: tag.name })),
  ];
}

/** The board's hidden columns, in column order. */
export function hiddenBoardColumns(
  group: TagGroup,
  tags: readonly Tag[],
  hidden: ReadonlySet<string>
): HiddenBoardColumn[] {
  return columnHeads(group, tags).filter((head) => hidden.has(head.key));
}

export function buildBoardColumns({
  group,
  tags,
  projects,
  groupTagIdOf,
  isVisible,
  prefs,
}: {
  group: TagGroup;
  /** The group's tags in position order. */
  tags: readonly Tag[];
  /** The projects the board shows, in card order. */
  projects: readonly Project[];
  /** A project's tag in the group, or null. */
  groupTagIdOf: (projectId: string) => string | null;
  /** Whether a search leaves the project's card showing. */
  isVisible: (projectId: string) => boolean;
  prefs: ResolvedColumnPrefs;
}): BoardLayout {
  const byKey = new Map<string, Project[]>(columnHeads(group, tags).map((head) => [head.key, []]));
  for (const project of projects) {
    const tagId = groupTagIdOf(project.id);
    // A tag that isn't one of the group's columns (gone since) counts as none.
    const column = (tagId !== null ? byKey.get(tagId) : undefined) ?? byKey.get(NO_TAG_COLUMN_KEY)!;
    column.push(project);
  }

  const columns: BoardColumn[] = [];
  const hidden: HiddenBoardColumn[] = [];
  let hiddenCount = 0;
  for (const head of columnHeads(group, tags)) {
    const cards = byKey.get(head.key)!;
    const count = cards.filter((project) => isVisible(project.id)).length;
    if (prefs.hidden.has(head.key)) {
      hidden.push(head);
      hiddenCount += count;
      continue;
    }
    columns.push({
      ...head,
      projects: cards,
      count,
      collapsed: prefs.collapsed.has(head.key),
    });
  }
  return { columns, hidden, hiddenCount };
}
