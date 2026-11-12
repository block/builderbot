import type { Branch, Project, ProjectRepo } from '../../../types';

/**
 * The stores the ⌘K palette reads, as rune state its component tests own.
 *
 * The test file's `vi.mock` factories serve these fields through the stores'
 * own shapes (projectsDataStore, navigation, projectFiltersStore). Because
 * they are `$state`, a test can change the data while the palette is open —
 * a project reordered, renamed, removed or starting to be removed — and the
 * mounted component follows, as it does in the app. Plain objects behind the
 * same getters are read once and never update.
 *
 * Fields are `$state.raw`, like the real stores': assign a new array, Map or
 * Set to change one.
 */
export type WorldRoute = { kind: string; projectId?: string };
export type WorldProjectLayer = { kind: 'project'; projectId: string } | { kind: 'repos' } | null;

class PaletteWorld {
  // projectsDataStore
  projects = $state.raw<Project[]>([]);
  repos = $state.raw(new Map<string, ProjectRepo[]>());
  branches = $state.raw(new Map<string, Branch[]>());
  /** Ids of projects whose removal is in flight. */
  deleting = $state.raw<ReadonlySet<string>>(new Set());
  loaded = $state(true);

  // projectFiltersStore: what the chips leave (null for no active filter),
  // and the filter Set, replaced on every change like the store's.
  filtered = $state.raw<Project[] | null>(null);
  activeFilters = $state.raw(new Set<string>());

  /** Repo badge short names by `owner/name` or `owner/name/subpath`. */
  shortNames = $state.raw<Record<string, string>>({});

  // navigation
  selectedProjectId = $state<string | null>(null);
  currentRoute = $state.raw<WorldRoute>({ kind: 'projects' });
  homeActive = $state(true);
  projectLayer = $state.raw<WorldProjectLayer>(null);
  /** What the diff view registered (registerDiffLeaveCheck), if anything. */
  diffLeaveCheck: (() => Promise<boolean>) | null = null;

  reset(projects: Project[]): void {
    this.projects = projects;
    this.repos = new Map();
    this.branches = new Map();
    this.deleting = new Set();
    this.loaded = true;
    this.filtered = null;
    this.activeFilters = new Set();
    this.shortNames = {};
    this.diffLeaveCheck = null;
    this.showHome();
  }

  /** The grid is on screen. A project layer may be retained under it. */
  showHome(retainedProjectId: string | null = null): void {
    this.currentRoute = { kind: 'projects' };
    this.selectedProjectId = null;
    this.homeActive = true;
    this.projectLayer = retainedProjectId
      ? { kind: 'project', projectId: retainedProjectId }
      : null;
  }

  /** A project is on screen, the sidebar beside it. */
  showProject(projectId: string): void {
    this.currentRoute = { kind: 'project', projectId };
    this.selectedProjectId = projectId;
    this.homeActive = false;
    this.projectLayer = { kind: 'project', projectId };
  }

  /** Settings or the diff view over the project the workspace is showing. */
  showOverProject(kind: 'settings' | 'diff', projectId: string): void {
    this.showProject(projectId);
    this.currentRoute = { kind, projectId };
  }
}

export const world = new PaletteWorld();
