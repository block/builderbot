/**
 * What a project's status icon shows: a user-chosen status option when one is
 * set, otherwise the status computed from its branches (aggregate PR state for
 * local projects, workspace state for remote ones).
 *
 * The option list is app-wide configuration (preferences), the selection is
 * per-project data (`Project.statusOverride`). Kept component-free so the
 * sidebar row, grid card and detail-page picker all resolve the same way and
 * the rules stay testable.
 */

import type { Branch, Project, WorkspaceStatus } from '../../types';
import { aggregateProjectPrStatus, projectHasCodeChanges } from '../../shared/utils';

/** A fixed palette, so overridden icons stay theme-aware and in-family. */
export type ProjectStatusColor = 'green' | 'red' | 'amber' | 'blue' | 'purple' | 'cyan' | 'gray';

export const PROJECT_STATUS_COLORS: ProjectStatusColor[] = [
  'green',
  'red',
  'amber',
  'blue',
  'purple',
  'cyan',
  'gray',
];

/** Dedicated tokens (defined in app.css) so status colors are themed on their own terms. */
export const PROJECT_STATUS_COLOR_VARS: Record<ProjectStatusColor, string> = {
  green: 'var(--project-status-green)',
  red: 'var(--project-status-red)',
  amber: 'var(--project-status-amber)',
  blue: 'var(--project-status-blue)',
  purple: 'var(--project-status-purple)',
  cyan: 'var(--project-status-cyan)',
  gray: 'var(--project-status-gray)',
};

export interface ProjectStatusOption {
  /** Stable slug for built-ins, a UUID for user-added options. */
  id: string;
  label: string;
  /** Kebab-case Lucide icon name. */
  icon: string;
  color: ProjectStatusColor;
}

export const DEFAULT_PROJECT_STATUS_OPTIONS: ProjectStatusOption[] = [
  { id: 'idea', label: 'Idea', icon: 'lightbulb', color: 'amber' },
  { id: 'in-progress', label: 'In progress', icon: 'circle-dot', color: 'blue' },
  { id: 'polishing', label: 'Polishing', icon: 'paintbrush', color: 'cyan' },
  { id: 'needs-review', label: 'Needs review', icon: 'eye', color: 'purple' },
  { id: 'done', label: 'Done', icon: 'circle-check', color: 'green' },
  { id: 'on-hold', label: 'On hold', icon: 'circle-pause', color: 'gray' },
  { id: 'blocked', label: 'Blocked', icon: 'octagon-minus', color: 'red' },
];

/** The icon a new, user-added status starts with. */
export const NEW_PROJECT_STATUS_ICON = 'circle-dot';

/**
 * Radio value for Default in the status menus (the detail-page picker and the
 * context-menu submenu); option ids are slugs or UUIDs, so it can't clash.
 */
export const DEFAULT_STATUS_MENU_VALUE = '__default__';

/**
 * Whether an option list is exactly the built-in defaults, in order. Used to
 * tell a no-op "Reset to defaults" from one that would discard user changes.
 */
export function isDefaultProjectStatusOptions(options: ProjectStatusOption[]): boolean {
  return (
    options.length === DEFAULT_PROJECT_STATUS_OPTIONS.length &&
    options.every((option, i) => {
      const d = DEFAULT_PROJECT_STATUS_OPTIONS[i];
      return (
        option.id === d.id &&
        option.label === d.label &&
        option.icon === d.icon &&
        option.color === d.color
      );
    })
  );
}

function isProjectStatusColor(value: unknown): value is ProjectStatusColor {
  return PROJECT_STATUS_COLORS.includes(value as ProjectStatusColor);
}

/**
 * Validate a stored option list. Malformed entries are dropped, duplicate ids
 * keep their first occurrence, and unknown colors become gray.
 *
 * An explicitly stored empty list is respected — a user who removes every
 * status must not get them back on restart. Returns null only when the value
 * is unusable (not an array, or nothing in it survived), so the caller falls
 * back to the defaults.
 */
export function normalizeProjectStatusOptions(raw: unknown): ProjectStatusOption[] | null {
  if (!Array.isArray(raw)) return null;
  if (raw.length === 0) return [];

  const seen = new Set<string>();
  const options: ProjectStatusOption[] = [];
  for (const entry of raw) {
    if (!entry || typeof entry !== 'object') continue;
    const { id, label, icon, color } = entry as Record<string, unknown>;
    if (typeof id !== 'string' || !id || seen.has(id)) continue;
    if (typeof label !== 'string') continue;
    seen.add(id);
    options.push({
      id,
      label,
      icon: typeof icon === 'string' && icon ? icon : NEW_PROJECT_STATUS_ICON,
      color: isProjectStatusColor(color) ? color : 'gray',
    });
  }
  return options.length > 0 ? options : null;
}

/** What an option reads as, even while its label is still blank. */
export function projectStatusOptionLabel(option: ProjectStatusOption): string {
  return option.label.trim() || 'Untitled status';
}

export type ProjectPrStatus = ReturnType<typeof aggregateProjectPrStatus>;

/** The status a project shows when no override applies. */
export type ComputedProjectStatus =
  | { kind: 'cloud'; workspaceStatus: WorkspaceStatus | null }
  | { kind: 'pr'; prStatus: ProjectPrStatus; hasCodeChanges: boolean }
  | { kind: 'placeholder' };

export type ResolvedProjectStatus =
  { kind: 'override'; option: ProjectStatusOption } | ComputedProjectStatus;

/**
 * The status computed from a project's branches. Remote projects know their
 * location from the project list alone, so they paint the cloud straight away;
 * local ones show a placeholder until their branches are hydrated.
 */
export function resolveComputedProjectStatus(
  project: Pick<Project, 'location'>,
  branches: Branch[],
  hydrated = true
): ComputedProjectStatus {
  if (project.location === 'remote') {
    return {
      kind: 'cloud',
      workspaceStatus: branches.find((b) => b.workspaceStatus)?.workspaceStatus ?? null,
    };
  }
  if (!hydrated) return { kind: 'placeholder' };
  return {
    kind: 'pr',
    prStatus: aggregateProjectPrStatus(branches),
    hasCodeChanges: projectHasCodeChanges(branches),
  };
}

/** The option a project has chosen, or null when it is on Default. */
export function findProjectStatusOverride(
  project: Pick<Project, 'statusOverride'>,
  options: ProjectStatusOption[]
): ProjectStatusOption | null {
  const id = project.statusOverride;
  if (!id) return null;
  // An id whose option was deleted in settings reads as Default.
  return options.find((o) => o.id === id) ?? null;
}

/**
 * What a project's status icon shows. A chosen option wins over both the PR
 * and the cloud status; it comes with the project list, so it needs no
 * hydration either.
 */
export function resolveProjectStatus(
  project: Pick<Project, 'location' | 'statusOverride'>,
  branches: Branch[],
  options: ProjectStatusOption[],
  hydrated = true
): ResolvedProjectStatus {
  const option = findProjectStatusOverride(project, options);
  if (option) return { kind: 'override', option };
  return resolveComputedProjectStatus(project, branches, hydrated);
}

function workspaceStatusLabel(status: WorkspaceStatus | null): string {
  switch (status) {
    case 'starting':
      return 'Workspace provisioning';
    case 'running':
      return 'Workspace running';
    case 'stopped':
      return 'Workspace stopped';
    case 'suspended':
      return 'Workspace suspended';
    case 'error':
      return 'Workspace error';
    default:
      return 'Remote workspace';
  }
}

/** Human-readable text for a computed status, as the picker's Default row shows it. */
export function computedStatusLabel(status: ComputedProjectStatus): string {
  switch (status.kind) {
    case 'cloud':
      return workspaceStatusLabel(status.workspaceStatus);
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
  return status.kind === 'override'
    ? projectStatusOptionLabel(status.option)
    : computedStatusLabel(status);
}
