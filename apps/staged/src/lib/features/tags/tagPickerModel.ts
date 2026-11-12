/**
 * The tag picker's rows (the `+ Tag` popover and the T shortcut), kept free
 * of runes and DOM so the filtering, the create row and keyboard movement are
 * unit-testable.
 *
 * Rows come in sections: one per group, in group order, whose tags act as
 * radio buttons (a project has at most one tag per group), then the ungrouped
 * tags, which act as checkboxes. A query keeps the tags whose name contains
 * it, or every tag of a group whose name does, ignoring case. When no tag's
 * name equals the query (ignoring case, in any scope), a create row offers to
 * make it an ungrouped tag. The highlight starts on a tag the query matches
 * before the create row, so Enter on `q4` picks `q4-launch` rather than making
 * a near-duplicate. "Manage tags…" always closes the list.
 */

import type { Tag } from '../../types';
import { MAX_TAG_NAME_CHARS, tagNameKey, type TagCatalog } from './tagState';

export type TagPickerRow =
  | {
      kind: 'tag';
      key: string;
      tag: Tag;
      /** Radio for a group's tag, checkbox for an ungrouped one. */
      mode: 'radio' | 'checkbox';
      checked: boolean;
    }
  | {
      kind: 'create';
      key: string;
      /** The trimmed query: the name the tag is created with. */
      name: string;
      /** The name is longer than the backend accepts. */
      tooLong: boolean;
    }
  | { kind: 'manage'; key: string };

export type TagPickerTagRow = Extract<TagPickerRow, { kind: 'tag' }>;

export interface TagPickerSection {
  /** The group's id, or null for the ungrouped tags. */
  groupId: string | null;
  label: string;
  rows: TagPickerTagRow[];
}

export interface TagPickerModel {
  sections: TagPickerSection[];
  create: Extract<TagPickerRow, { kind: 'create' }> | null;
  manage: Extract<TagPickerRow, { kind: 'manage' }>;
  /** Every row in keyboard order: the sections' tags, the create row, Manage. */
  rows: TagPickerRow[];
  /** Where the highlight starts for this query (see defaultActiveIndex). */
  defaultActive: number;
}

/** Heading for the ungrouped tags' section. */
export const UNGROUPED_SECTION_LABEL = 'Tags';

const MANAGE_ROW: TagPickerModel['manage'] = { kind: 'manage', key: 'manage' };

/** Case-insensitive containment, folding the way names are compared. */
function contains(haystack: string, needle: string): boolean {
  return haystack.toLowerCase().includes(needle);
}

export function buildTagPickerModel(
  catalog: TagCatalog,
  tagIds: ReadonlySet<string>,
  query: string
): TagPickerModel {
  const trimmed = query.trim();
  const needle = trimmed.toLowerCase();
  const sections: TagPickerSection[] = [];

  const scopes: { groupId: string | null; label: string }[] = [
    ...catalog.groups.map((group) => ({ groupId: group.id, label: group.name })),
    { groupId: null, label: UNGROUPED_SECTION_LABEL },
  ];
  for (const { groupId, label } of scopes) {
    const tags = catalog.tagsByGroup.get(groupId) ?? [];
    const groupMatches = groupId !== null && needle !== '' && contains(label, needle);
    const shown = needle && !groupMatches ? tags.filter((t) => contains(t.name, needle)) : tags;
    if (shown.length === 0) continue;
    sections.push({
      groupId,
      label,
      rows: shown.map((tag) => ({
        kind: 'tag',
        key: `tag:${tag.id}`,
        tag,
        mode: groupId === null ? 'checkbox' : 'radio',
        checked: tagIds.has(tag.id),
      })),
    });
  }

  let create: TagPickerModel['create'] = null;
  let exactIndex = -1;
  const tagRows = sections.flatMap((section) => section.rows);
  if (trimmed) {
    const key = tagNameKey(trimmed);
    exactIndex = tagRows.findIndex((row) => tagNameKey(row.tag.name) === key);
    // A tag named exactly this always shows (its name contains the query), so
    // a miss here means no tag in any scope has the name.
    if (exactIndex < 0) {
      create = {
        kind: 'create',
        key: 'create',
        name: trimmed,
        tooLong: [...trimmed].length > MAX_TAG_NAME_CHARS,
      };
    }
  }

  const rows: TagPickerRow[] = [...tagRows, ...(create ? [create] : []), MANAGE_ROW];
  return {
    sections,
    create,
    manage: MANAGE_ROW,
    rows,
    defaultActive: defaultActiveIndex(rows, exactIndex, trimmed !== ''),
  };
}

/**
 * Where the highlight starts: on a query, the tag it names exactly, else the
 * first tag it matches (an existing tag beats a near-duplicate), else the
 * create row, so Enter creates only when nothing matches; with no query, the
 * first row.
 */
function defaultActiveIndex(rows: TagPickerRow[], exactIndex: number, hasQuery: boolean): number {
  if (!hasQuery) return 0;
  if (exactIndex >= 0) return exactIndex;
  const firstTag = rows.findIndex((row) => row.kind === 'tag');
  if (firstTag >= 0) return firstTag;
  const createIndex = rows.findIndex((row) => row.kind === 'create');
  return createIndex >= 0 ? createIndex : 0;
}

/** The highlight after an Up (-1) or Down (+1) press, wrapping at the ends. */
export function moveActiveIndex(index: number, delta: 1 | -1, count: number): number {
  if (count === 0) return 0;
  return (((index + delta) % count) + count) % count;
}
