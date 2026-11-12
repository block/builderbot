/**
 * What a project's status icon shows: its tag in the icon group when it has
 * one, otherwise the status computed from its branches (their aggregate PR
 * state). Local and remote projects resolve alike; a remote project's location
 * is marked separately (RemoteProjectIcon), not by its status icon.
 *
 * Tags are app-wide catalog data, a project's tags per-project data (both in
 * tagsStore). Kept component-free so the sidebar row, grid card and the
 * header's icon group pill all resolve the same way and the rules stay
 * testable.
 */

import type { Branch, Tag } from '../../types';
import { aggregateProjectPrStatus, projectHasCodeChanges } from '../../shared/utils';

export type ProjectPrStatus = ReturnType<typeof aggregateProjectPrStatus>;

/** The status a project shows when no icon-group tag applies. */
export type ComputedProjectStatus =
  { kind: 'pr'; prStatus: ProjectPrStatus; hasCodeChanges: boolean } | { kind: 'placeholder' };

export type ResolvedProjectStatus = { kind: 'tag'; tag: Tag } | ComputedProjectStatus;

/**
 * The status computed from a project's branches: a placeholder until they are
 * hydrated, then their aggregate PR state.
 */
export function resolveComputedProjectStatus(
  branches: Branch[],
  hydrated = true
): ComputedProjectStatus {
  if (!hydrated) return { kind: 'placeholder' };
  return {
    kind: 'pr',
    prStatus: aggregateProjectPrStatus(branches),
    hasCodeChanges: projectHasCodeChanges(branches),
  };
}

/**
 * What a project's status icon shows. Its icon-group tag wins over the PR
 * status. Until the tags have loaded, whether one applies is unknown, so the
 * icon stays a placeholder rather than flashing the PR status first.
 */
export function resolveProjectStatus(
  iconTag: Tag | null,
  branches: Branch[],
  hydrated = true,
  tagsLoaded = true
): ResolvedProjectStatus {
  if (!tagsLoaded) return { kind: 'placeholder' };
  if (iconTag) return { kind: 'tag', tag: iconTag };
  return resolveComputedProjectStatus(branches, hydrated);
}

/** Human-readable text for a computed status, as the icon group's Default row shows it. */
export function computedStatusLabel(status: ComputedProjectStatus): string {
  switch (status.kind) {
    case 'placeholder':
      return 'Loading…';
    case 'pr':
      switch (status.prStatus) {
        case 'merged':
          return 'Merged';
        case 'open':
          return 'PR open';
        case 'closed':
          return 'PR closed';
        case 'conflict':
          return 'Merge conflict';
        case 'checks_failing':
          return 'Checks failing';
        default:
          return status.hasCodeChanges ? 'No PR yet' : 'No changes yet';
      }
  }
}

/** Human-readable text for whatever a project's status icon shows. */
export function resolvedStatusLabel(status: ResolvedProjectStatus): string {
  return status.kind === 'tag' ? status.tag.name : computedStatusLabel(status);
}
