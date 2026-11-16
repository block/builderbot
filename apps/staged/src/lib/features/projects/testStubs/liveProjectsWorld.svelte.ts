import type { Project } from '../../../types';

/**
 * The project list as rune state a component test owns, served through
 * projectsDataStore's shape by the test file's mock factory. Because it is
 * `$state`, a test can create a project or start deleting one and the stores
 * derived from the list follow, as they do in the app; a plain array behind
 * the same getter is read once and never updates.
 *
 * `projects` is `$state.raw`, like the real store's: assign a new array to
 * change it. `liveProjects` is derived the way the real store derives it,
 * so with nothing deleting it is `projects` itself.
 */
class LiveProjectsWorld {
  projects = $state.raw<Project[]>([]);
  /** Ids of projects whose removal is in flight. */
  deleting = $state.raw<ReadonlySet<string>>(new Set());

  readonly liveProjects: Project[] = $derived.by(() => {
    const deleting = this.deleting;
    if (deleting.size === 0) return this.projects;
    return this.projects.filter((project) => !deleting.has(project.id));
  });
}

export const world = new LiveProjectsWorld();
