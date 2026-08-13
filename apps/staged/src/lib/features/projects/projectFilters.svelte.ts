/**
 * Shared project-filter state.
 *
 * Module-scoped runes store (following the projectsData store pattern) that
 * owns the filter selection ProjectsList and ProjectsSidebar both render.
 * Because it lives at module scope, the active filters survive navigating
 * into a project and back for the app session — deliberately not persisted
 * to disk, matching how nothing about the filter UI persisted before.
 *
 * The derived getters read the global data stores directly; the underlying
 * computations are exported as plain functions taking data as arguments so
 * the repo-fallback rules (repos list vs. legacy project.githubRepo,
 * headRepo ?? githubRepo) stay unit-testable without store plumbing.
 */

import type { Project, ProjectRepo } from '../../types';
import { projectsDataStore } from '../../stores/projectsData.svelte';
import { projectStateStore } from '../../stores/projectState.svelte';
import { getProjectStatus } from './projectStatus';

/** A repo+subpath a filter chip can target. */
export interface RepoFilterRef {
  repo: string;
  subpath: string;
}

export type FilterKind = 'unread' | 'running' | RepoFilterRef;

/** A repo chip entry: the target plus how many projects show that repo. */
export interface RepoFilter extends RepoFilterRef {
  count: number;
}

export function filterKey(filter: FilterKind): string {
  if (typeof filter === 'string') return filter;
  return `repo:${filter.repo}:${filter.subpath}`;
}

/** Inverse of filterKey for repo filters; null for status keys. GitHub repo
 *  names can't contain a colon, so the first one ends the repo segment. */
export function parseRepoFilterKey(key: string): RepoFilterRef | null {
  if (!key.startsWith('repo:')) return null;
  const rest = key.slice('repo:'.length);
  const separator = rest.indexOf(':');
  if (separator === -1) return null;
  return { repo: rest.slice(0, separator), subpath: rest.slice(separator + 1) };
}

/**
 * Unique repo+subpath entries with project counts, sorted alphabetically by
 * full display string. Projects with a hydrated repos list contribute every
 * repo (displayed as headRepo ?? githubRepo); projects without one fall back
 * to the legacy project.githubRepo field.
 */
export function computeRepoFilters(
  projects: Project[],
  reposByProject: Map<string, ProjectRepo[]>
): RepoFilter[] {
  const counts = new Map<string, RepoFilter>();
  const add = (repo: string, subpath: string) => {
    const key = `${repo}:${subpath}`;
    const entry = counts.get(key);
    if (entry) {
      entry.count++;
    } else {
      counts.set(key, { repo, subpath, count: 1 });
    }
  };
  for (const project of projects) {
    const repos = reposByProject.get(project.id) ?? [];
    if (repos.length > 0) {
      for (const r of repos) {
        add(r.headRepo ?? r.githubRepo, r.subpath ?? '');
      }
    } else if (project.githubRepo) {
      add(project.githubRepo, project.subpath ?? '');
    }
  }
  return [...counts.values()].sort((a, b) => {
    const aDisplay = a.subpath ? `${a.repo}/${a.subpath}` : a.repo;
    const bDisplay = b.subpath ? `${b.repo}/${b.subpath}` : b.repo;
    return aDisplay.localeCompare(bDisplay);
  });
}

export function hasRepoFilterKeys(activeFilters: Set<string>): boolean {
  return [...activeFilters].some((key) => key !== 'unread' && key !== 'running');
}

/**
 * Apply the active filter set: status filters AND with each other and with
 * repo filters; repo filters OR with each other. The unread/running checks
 * are injected so this stays pure.
 */
export function filterProjects(
  projects: Project[],
  activeFilters: Set<string>,
  reposByProject: Map<string, ProjectRepo[]>,
  isUnread: (projectId: string) => boolean,
  isRunning: (projectId: string) => boolean
): Project[] {
  if (activeFilters.size === 0) return projects;
  const hasRepoFilters = hasRepoFilterKeys(activeFilters);
  return projects.filter((p) => {
    if (activeFilters.has('unread') && !isUnread(p.id)) return false;
    if (activeFilters.has('running') && !isRunning(p.id)) return false;
    if (!hasRepoFilters) return true;
    const repos = reposByProject.get(p.id) ?? [];
    if (repos.length > 0) {
      return repos.some((r) =>
        activeFilters.has(filterKey({ repo: r.headRepo ?? r.githubRepo, subpath: r.subpath ?? '' }))
      );
    }
    if (p.githubRepo) {
      return activeFilters.has(filterKey({ repo: p.githubRepo, subpath: p.subpath ?? '' }));
    }
    return false;
  });
}

/**
 * Chip click semantics: a plain click selects the filter exclusively, and
 * clicking the only active filter deselects it (back to showing all);
 * shift-click toggles the filter within the current set.
 */
export function toggleFilterKey(
  activeFilters: Set<string>,
  key: string,
  additive: boolean
): Set<string> {
  if (additive) {
    const next = new Set(activeFilters);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    return next;
  }
  if (activeFilters.size === 1 && activeFilters.has(key)) {
    return new Set();
  }
  return new Set([key]);
}

class ProjectFiltersStore {
  private _activeFilters = $state<Set<string>>(new Set());

  // ── Reactive reads ──

  get activeFilters(): Set<string> {
    return this._activeFilters;
  }

  get hasActiveFilters(): boolean {
    return this._activeFilters.size > 0;
  }

  /** True when any repo (non-status) filter is active. */
  get hasRepoFilters(): boolean {
    return hasRepoFilterKeys(this._activeFilters);
  }

  get repoFilters(): RepoFilter[] {
    return computeRepoFilters(projectsDataStore.projects, projectsDataStore.reposByProject);
  }

  /** The active repo filters, parsed from their keys. Kept independent of
   *  repoFilters so a stale-but-active selection still renders a summary. */
  get activeRepoFilters(): RepoFilterRef[] {
    return [...this._activeFilters]
      .map(parseRepoFilterKey)
      .filter((f): f is RepoFilterRef => f !== null);
  }

  get unreadCount(): number {
    return projectsDataStore.projects.filter((p) => projectStateStore.isUnread(p.id)).length;
  }

  get runningCount(): number {
    return projectsDataStore.projects.filter((p) => this.isProjectRunning(p.id)).length;
  }

  get filteredProjects(): Project[] {
    return filterProjects(
      projectsDataStore.projects,
      this._activeFilters,
      projectsDataStore.reposByProject,
      (projectId) => projectStateStore.isUnread(projectId),
      (projectId) => this.isProjectRunning(projectId)
    );
  }

  // ── Mutations ──

  isFilterActive(filter: FilterKind): boolean {
    return this._activeFilters.has(filterKey(filter));
  }

  toggleFilter(filter: FilterKind, event?: MouseEvent): void {
    this._activeFilters = toggleFilterKey(
      this._activeFilters,
      filterKey(filter),
      event?.shiftKey ?? false
    );
  }

  clearFilters(): void {
    if (this._activeFilters.size === 0) return;
    this._activeFilters = new Set();
  }

  private isProjectRunning(projectId: string): boolean {
    const status = getProjectStatus(
      projectId,
      projectsDataStore.deletingProjectNames,
      projectsDataStore.branchesByProject.get(projectId) || []
    );
    return status.kind === 'running' || status.kind === 'runAction';
  }
}

export const projectFiltersStore = new ProjectFiltersStore();
