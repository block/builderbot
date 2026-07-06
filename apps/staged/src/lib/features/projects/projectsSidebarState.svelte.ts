import type { Project } from '../../types';
import { getStoreValue, setStoreValue } from '../../shared/persistentStore';

const SIDEBAR_WIDTH_KEY = 'projects-sidebar-width';

export const SIDEBAR_DEFAULT_WIDTH = 280;
export const SIDEBAR_MIN_WIDTH = 220;
export const SIDEBAR_MAX_WIDTH = 520;

function clampWidth(width: number): number {
  return Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, width));
}

/**
 * Shared reactive list of projects.
 *
 * Written by ProjectsList when it loads/reloads projects, read by
 * navigation shortcuts so they don't need a separate IPC round-trip.
 */
export const projectsList = $state<{ current: Project[] }>({ current: [] });

export const projectsSidebarState = $state({
  width: SIDEBAR_DEFAULT_WIDTH,
  hydrated: false,
  hasProjects: true,
  scrollTop: 0,
});

export async function hydrateProjectsSidebarState(): Promise<void> {
  if (projectsSidebarState.hydrated) return;

  const savedWidth = await getStoreValue<number>(SIDEBAR_WIDTH_KEY);

  if (typeof savedWidth === 'number' && Number.isFinite(savedWidth)) {
    projectsSidebarState.width = clampWidth(savedWidth);
  }

  projectsSidebarState.hydrated = true;
}

export function setProjectsSidebarWidth(width: number, persist = true): void {
  const clamped = clampWidth(width);
  if (projectsSidebarState.width !== clamped) {
    projectsSidebarState.width = clamped;
  }
  if (persist) {
    void setStoreValue(SIDEBAR_WIDTH_KEY, clamped);
  }
}

export function setProjects(projects: Project[]): void {
  projectsList.current = projects;
  projectsSidebarState.hasProjects = projects.length > 0;
}

export function setProjectsSidebarScrollTop(scrollTop: number): void {
  if (!Number.isFinite(scrollTop)) return;
  projectsSidebarState.scrollTop = Math.max(0, scrollTop);
}
