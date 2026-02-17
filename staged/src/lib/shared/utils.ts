/**
 * Shared utility functions.
 */

import type { Project } from '../types';

/** Display name for a project: repo basename + optional subpath. */
export function projectDisplayName(p: Project): string {
  return p.name;
}
