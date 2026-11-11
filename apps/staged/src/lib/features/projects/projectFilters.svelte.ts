/**
 * Shared project-filter state.
 *
 * Module-scoped runes store (following the projectsData store pattern) that
 * owns the filter selection ProjectsList and ProjectsSidebar both render.
 * Because it lives at module scope, the active filters survive navigating
 * into a project and back for the app session — deliberately not persisted
 * to disk, matching how nothing about the filter UI persisted before.
 *
 * The $derived fields read the global data stores directly; the underlying
 * computations are exported as plain functions taking data as arguments so
 * the repo-fallback rules (repos list vs. legacy project.githubRepo,
 * headRepo ?? githubRepo) and the tag rules (TagFilterContext) stay
 * unit-testable without store plumbing.
 *
 * Tag filter keys name a tag id and nothing else. Which group a tag is in is
 * read from the catalog each time the filters apply, and a key naming a tag
 * the catalog no longer has (deleted, here or in another window) is ignored
 * everywhere: it filters nothing, counts as no filter, and goes on the next
 * toggle. So there's nothing to prune, and tagsStore never has to know about
 * this store.
 */

import type { Project, ProjectRepo, Tag, TagGroup } from '../../types';
import { projectsDataStore } from '../../stores/projectsData.svelte';
import { projectStateStore } from '../../stores/projectState.svelte';
import { tagsStore } from '../../stores/tags.svelte';
import type { TagCatalog } from '../tags/tagState';
import { UNGROUPED_SECTION_LABEL } from '../tags/tagPickerModel';
import { getProjectStatus } from './projectStatus';
import { workspaceLifecycle } from './workspaceLifecycle.svelte';

/** A repo+subpath a filter chip can target. */
export interface RepoFilterRef {
  repo: string;
  subpath: string;
}

/** A tag a filter chip can target. */
export interface TagFilterRef {
  tagId: string;
}

export type FilterKind = 'unread' | 'running' | RepoFilterRef | TagFilterRef;

/** A repo chip entry: the target plus how many projects show that repo. */
export interface RepoFilter extends RepoFilterRef {
  count: number;
}

/** A tag chip entry: the tag, its group (null if ungrouped) and its live-project count. */
export interface TagFilter {
  tag: Tag;
  group: TagGroup | null;
  count: number;
}

/** The tag chips under one heading: a group's, or the ungrouped tags'. */
export interface TagFilterSection {
  /** `group:<id>`, or `ungrouped`. */
  key: string;
  label: string;
  filters: TagFilter[];
}

export function filterKey(filter: FilterKind): string {
  if (typeof filter === 'string') return filter;
  if ('tagId' in filter) return `tag:${filter.tagId}`;
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

/** Inverse of filterKey for tag filters; null for any other key. By prefix,
 *  like parseRepoFilterKey, so the two can never both claim a key. */
export function parseTagFilterKey(key: string): TagFilterRef | null {
  if (!key.startsWith('tag:')) return null;
  const tagId = key.slice('tag:'.length);
  return tagId ? { tagId } : null;
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

/**
 * True when the set holds any repo filter. Classifies through
 * parseRepoFilterKey so this and the repo matching in filterProjects agree by
 * construction — classifying by exclusion instead ("anything that isn't
 * unread or running") would silently promote a future third status filter to
 * a repo filter that no project's repos can match, filtering everything out.
 */
export function hasRepoFilterKeys(activeFilters: Set<string>): boolean {
  for (const key of activeFilters) {
    if (parseRepoFilterKey(key) !== null) return true;
  }
  return false;
}

/** What the tag filters read, injected so filterProjects stays pure. */
export interface TagFilterContext {
  /** The project's tags, pending writes included. */
  tagIdsFor(projectId: string): ReadonlySet<string>;
  /** The tag's group id, or null for an ungrouped tag. Only asked about known tags. */
  groupOf(tagId: string): string | null;
  /** The tags in the catalog. A key naming any other tag is ignored. */
  knownTagIds: { has(tagId: string): boolean };
}

const NO_TAG_IDS: ReadonlySet<string> = new Set();

/** No catalog: every tag key is unknown, so none of them filters. */
const NO_TAGS: TagFilterContext = {
  tagIdsFor: () => NO_TAG_IDS,
  groupOf: () => null,
  knownTagIds: NO_TAG_IDS,
};

/**
 * The active tag filters as clauses, each a list of tag ids: a project passes
 * when it has at least one tag from every clause. Tags in one group share a
 * clause (a project holds at most one tag per group, so they can only OR), and
 * each ungrouped tag is its own clause (they AND). Grouped by the catalog's
 * current groups, not the ones the tags had when their chips were clicked,
 * since a tag can move between groups while its filter is on. Keys naming
 * unknown tags are skipped. Clauses come in the order their first key was
 * added.
 */
export function tagFilterClauses(
  activeFilters: ReadonlySet<string>,
  tags: TagFilterContext
): string[][] {
  const clauses: string[][] = [];
  const byGroup = new Map<string, string[]>();
  for (const key of activeFilters) {
    const ref = parseTagFilterKey(key);
    if (!ref || !tags.knownTagIds.has(ref.tagId)) continue;
    const groupId = tags.groupOf(ref.tagId);
    if (groupId === null) {
      clauses.push([ref.tagId]);
      continue;
    }
    const clause = byGroup.get(groupId);
    if (clause) {
      clause.push(ref.tagId);
    } else {
      const fresh = [ref.tagId];
      byGroup.set(groupId, fresh);
      clauses.push(fresh);
    }
  }
  return clauses;
}

function matchesRepoFilters(
  project: Project,
  activeFilters: ReadonlySet<string>,
  reposByProject: Map<string, ProjectRepo[]>
): boolean {
  const repos = reposByProject.get(project.id) ?? [];
  if (repos.length > 0) {
    return repos.some((r) =>
      activeFilters.has(filterKey({ repo: r.headRepo ?? r.githubRepo, subpath: r.subpath ?? '' }))
    );
  }
  if (project.githubRepo) {
    return activeFilters.has(
      filterKey({ repo: project.githubRepo, subpath: project.subpath ?? '' })
    );
  }
  return false;
}

/**
 * Apply the active filter set. Unread and running AND with everything; repo
 * filters OR with each other; tag filters OR within a group, groups AND with
 * each other, and each ungrouped tag ANDs (tagFilterClauses). The unread,
 * running and tag checks are injected so this stays pure. A set holding only
 * keys nothing can match by (a deleted tag's) returns `projects` itself.
 */
export function filterProjects(
  projects: Project[],
  activeFilters: Set<string>,
  reposByProject: Map<string, ProjectRepo[]>,
  isUnread: (projectId: string) => boolean,
  isRunning: (projectId: string) => boolean,
  tags: TagFilterContext = NO_TAGS
): Project[] {
  if (activeFilters.size === 0) return projects;
  const unread = activeFilters.has('unread');
  const running = activeFilters.has('running');
  const hasRepoFilters = hasRepoFilterKeys(activeFilters);
  const tagClauses = tagFilterClauses(activeFilters, tags);
  if (!unread && !running && !hasRepoFilters && tagClauses.length === 0) return projects;
  return projects.filter((p) => {
    if (unread && !isUnread(p.id)) return false;
    if (running && !isRunning(p.id)) return false;
    if (tagClauses.length > 0) {
      const tagIds = tags.tagIdsFor(p.id);
      if (!tagClauses.every((clause) => clause.some((id) => tagIds.has(id)))) return false;
    }
    return !hasRepoFilters || matchesRepoFilters(p, activeFilters, reposByProject);
  });
}

/**
 * `activeFilters` without the tag keys naming tags the catalog no longer has:
 * the filters that still do something, which is what the filter UI counts and
 * offers to clear. filterProjects already ignores those keys; without this
 * a deleted tag would leave an invisible filter behind that keeps Clear and
 * the menu's count up. Returns `activeFilters` itself when every key is live.
 */
export function effectiveFilterKeys(
  activeFilters: Set<string>,
  knownTagIds: { has(tagId: string): boolean }
): Set<string> {
  const isStale = (key: string) => {
    const ref = parseTagFilterKey(key);
    return ref !== null && !knownTagIds.has(ref.tagId);
  };
  if (![...activeFilters].some(isStale)) return activeFilters;
  return new Set([...activeFilters].filter((key) => !isStale(key)));
}

/**
 * The tag chips, in catalog order (groups in order, then the ungrouped
 * tags): every tag a live project carries, with its count, plus any tag whose
 * filter is on even when no project carries it any more. That chip stays so
 * the filter can be clicked off where it was clicked on, rather than leaving
 * only Clear.
 */
export function computeTagFilters(
  catalog: TagCatalog,
  usage: ReadonlyMap<string, number>,
  activeFilters: ReadonlySet<string>
): TagFilter[] {
  const filters: TagFilter[] = [];
  for (const group of [...catalog.groups, null]) {
    for (const tag of catalog.tagsByGroup.get(group?.id ?? null) ?? []) {
      const count = usage.get(tag.id) ?? 0;
      if (count > 0 || activeFilters.has(filterKey({ tagId: tag.id }))) {
        filters.push({ tag, group, count });
      }
    }
  }
  return filters;
}

/**
 * `filters` (computeTagFilters' order) under headings: one per group, named
 * after it, then the ungrouped tags under "Tags", as in the tag picker.
 * Headings with no chips are left out.
 */
export function sectionTagFilters(filters: readonly TagFilter[]): TagFilterSection[] {
  const sections: TagFilterSection[] = [];
  for (const filter of filters) {
    const key = filter.group ? `group:${filter.group.id}` : 'ungrouped';
    let section = sections[sections.length - 1];
    if (section?.key !== key) {
      section = { key, label: filter.group?.name ?? UNGROUPED_SECTION_LABEL, filters: [] };
      sections.push(section);
    }
    section.filters.push(filter);
  }
  return sections;
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

  /** The tag filters' view of tagsStore, read inside filteredProjects so it tracks it. */
  private readonly tagContext: TagFilterContext = {
    tagIdsFor: (projectId) => tagsStore.tagIdsFor(projectId),
    groupOf: (tagId) => tagsStore.tagsById.get(tagId)?.groupId ?? null,
    knownTagIds: { has: (tagId) => tagsStore.tagsById.has(tagId) },
  };

  // ── Reactive reads ──
  //
  // $derived fields rather than getters so every consumer shares one
  // computation instead of recomputing per read: filteredProjects is read by
  // the landing grid, the sidebar's project list and the sidebar row's match
  // count, and each pass would otherwise call getProjectStatus for every
  // project when `running` is active. The counts are read twice per chip.
  //
  // All of them read liveProjects, so a project being deleted drops out of
  // the chips' counts and repo filters as its card leaves the list.

  /**
   * The selection as toggled, stale tag keys included. Replaced, never
   * mutated, on every toggle or clear and only then, so its identity is the
   * signal that the user changed the filters (the grid's sticky card and
   * searchOrderHold go by it).
   */
  get activeFilters(): Set<string> {
    return this._activeFilters;
  }

  /** The selection minus keys naming deleted tags (effectiveFilterKeys). */
  effectiveActiveFilters: Set<string> = $derived(
    effectiveFilterKeys(this._activeFilters, tagsStore.tagsById)
  );

  hasActiveFilters = $derived(this.effectiveActiveFilters.size > 0);

  repoFilters: RepoFilter[] = $derived.by(() =>
    computeRepoFilters(projectsDataStore.liveProjects, projectsDataStore.reposByProject)
  );

  /** The active repo filters, parsed from their keys. Kept independent of
   *  repoFilters so a stale-but-active selection still renders a summary. */
  activeRepoFilters: RepoFilterRef[] = $derived.by(() =>
    [...this._activeFilters].map(parseRepoFilterKey).filter((f): f is RepoFilterRef => f !== null)
  );

  unreadCount = $derived.by(
    () => projectsDataStore.liveProjects.filter((p) => projectStateStore.isUnread(p.id)).length
  );

  runningCount = $derived.by(
    () => projectsDataStore.liveProjects.filter((p) => this.isProjectRunning(p.id)).length
  );

  /** Tag chips: the tags live projects carry, and any with its filter on (computeTagFilters). */
  tagFilters: TagFilter[] = $derived.by(() =>
    computeTagFilters(tagsStore.catalog, tagsStore.usageCount, this.effectiveActiveFilters)
  );

  /** tagFilters under a heading per group, then the ungrouped tags (the filter menu). */
  tagFilterSections: TagFilterSection[] = $derived(sectionTagFilters(this.tagFilters));

  filteredProjects: Project[] = $derived.by(() =>
    filterProjects(
      projectsDataStore.liveProjects,
      this._activeFilters,
      projectsDataStore.reposByProject,
      (projectId) => projectStateStore.isUnread(projectId),
      (projectId) => this.isProjectRunning(projectId),
      this.tagContext
    )
  );

  // ── Mutations ──

  isFilterActive(filter: FilterKind): boolean {
    return this._activeFilters.has(filterKey(filter));
  }

  /**
   * Chips pass their click event (shift = additive); callers without a
   * click convention, like the sidebar's filter menu, state the intent
   * directly with `{ additive }`. No argument means an exclusive select.
   */
  toggleFilter(filter: FilterKind, intent?: MouseEvent | { additive: boolean }): void {
    const additive =
      intent === undefined ? false : 'additive' in intent ? intent.additive : intent.shiftKey;
    // From the filters still in effect, so a deleted tag's key goes now: a
    // plain click on the one chip showing as active then clears it, rather
    // than keeping it alongside a key nobody can see.
    const current = effectiveFilterKeys(this._activeFilters, tagsStore.tagsById);
    this._activeFilters = toggleFilterKey(current, filterKey(filter), additive);
  }

  clearFilters(): void {
    if (this._activeFilters.size === 0) return;
    this._activeFilters = new Set();
  }

  private isProjectRunning(projectId: string): boolean {
    const status = getProjectStatus(
      projectId,
      projectsDataStore.deletingProjectNames,
      projectsDataStore.branchesByProject.get(projectId) || [],
      workspaceLifecycle.getWorktreeErrors()
    );
    // A disconnected Blox session counts: its agent may still be working.
    return (
      status.kind === 'running' ||
      status.kind === 'backgroundHold' ||
      status.kind === 'disconnected' ||
      status.kind === 'runAction'
    );
  }
}

export const projectFiltersStore = new ProjectFiltersStore();
