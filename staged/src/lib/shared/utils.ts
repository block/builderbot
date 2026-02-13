/**
 * Shared utility functions.
 */

import type { Project } from '../types';

/** Display name for a project: repo basename + optional subpath. */
export function projectDisplayName(p: Project): string {
  const repoName = p.githubRepo.split('/').pop() || p.githubRepo;
  return p.subpath ? `${repoName}/${p.subpath}` : repoName;
}
