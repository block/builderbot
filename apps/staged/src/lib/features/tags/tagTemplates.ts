/**
 * Starting points for a new tag group in Settings › Tags. A template is a
 * name and the tags to create with it; the backend assigns fresh ids, so a
 * second Status group shares nothing with the first but the look.
 */

import type { NewTag, TagGroup } from '../../types';
import { numberedName, tagNameKey } from './tagState';

export type TagGroupTemplateId = 'status' | 'priority' | 'blank';

export interface TagGroupTemplate {
  id: TagGroupTemplateId;
  /** The template's name in the New group menu. */
  label: string;
  /** The new group's name, numbered when a group already has it. */
  name: string;
  /** What the template holds, for the New group menu. */
  description: string;
  tags: readonly NewTag[];
}

export const TAG_GROUP_TEMPLATES: readonly TagGroupTemplate[] = [
  {
    id: 'status',
    label: 'Status',
    name: 'Status',
    description: 'Idea, In progress, … Done',
    // The project statuses Staged shipped before tags.
    tags: [
      { name: 'Idea', icon: 'lightbulb', color: 'amber' },
      { name: 'In progress', icon: 'circle-dot', color: 'blue' },
      { name: 'Polishing', icon: 'paintbrush', color: 'cyan' },
      { name: 'Needs review', icon: 'eye', color: 'purple' },
      { name: 'Done', icon: 'circle-check', color: 'green' },
      { name: 'On hold', icon: 'circle-pause', color: 'gray' },
      { name: 'Blocked', icon: 'octagon-minus', color: 'red' },
    ],
  },
  {
    id: 'priority',
    label: 'Priority',
    name: 'Priority',
    description: 'P0 to P3',
    tags: [
      { name: 'P0', icon: 'chevrons-up', color: 'red' },
      { name: 'P1', icon: 'chevron-up', color: 'amber' },
      { name: 'P2', icon: 'minus', color: 'blue' },
      { name: 'P3', icon: 'chevron-down', color: 'gray' },
    ],
  },
  {
    id: 'blank',
    label: 'Blank',
    name: 'New group',
    description: 'No tags yet',
    tags: [],
  },
];

/** The template's name, or "Name 2", "Name 3", … if a group already has it. */
export function freeGroupName(
  template: Pick<TagGroupTemplate, 'name'>,
  groups: readonly Pick<TagGroup, 'name'>[]
): string {
  return numberedName(template.name, new Set(groups.map((group) => tagNameKey(group.name))));
}
