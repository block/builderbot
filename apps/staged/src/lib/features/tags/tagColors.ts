/**
 * The fixed tag palette. Tags pick one of seven names rather than a free
 * colour, so they stay theme-aware and in-family; each name maps to a
 * dedicated `--project-status-*` token in app.css (named for the project
 * status these colours first served).
 */

import type { TagColor } from '../../types';

/** Swatch order in pickers. */
export const TAG_COLORS: TagColor[] = ['green', 'red', 'amber', 'blue', 'purple', 'cyan', 'gray'];

export const TAG_COLOR_VARS: Record<TagColor, string> = {
  green: 'var(--project-status-green)',
  red: 'var(--project-status-red)',
  amber: 'var(--project-status-amber)',
  blue: 'var(--project-status-blue)',
  purple: 'var(--project-status-purple)',
  cyan: 'var(--project-status-cyan)',
  gray: 'var(--project-status-gray)',
};
