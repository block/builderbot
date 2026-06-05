/**
 * Lightweight client-side navigation state.
 *
 * Controls which view is shown in the main content area:
 * - `activeView === 'settings'` → Settings page
 * - `activeView === 'workspace'` + `selectedProjectId === null` → ProjectsList (landing page)
 * - `activeView === 'workspace'` + `selectedProjectId === <id>` → ProjectHome filtered to that project
 * - `settingsSection` selects which settings panel is shown
 *
 * The last viewed project is persisted so the user returns to it on relaunch.
 */

import { getStoreValue, setStoreValue } from '../../shared/persistentStore';
import * as commands from '../../api/commands';
import { projectStateStore } from '../../stores/projectState.svelte';
import { projectsList } from '../projects/projectsSidebarState.svelte';
import { requestProjectsListRestore } from '../projects/projectsListViewState.svelte';
import { reposUiEnabled } from '../../featureFlags';

const LAST_PROJECT_STORE_KEY = 'last-viewed-project';

export type SettingsSection = 'general' | 'repo' | 'keyboard' | 'doctor';

export const navigation = $state({
  activeView: 'workspace' as 'workspace' | 'settings',
  selectedProjectId: null as string | null,
  showReposList: false,
  settingsSection: 'general' as SettingsSection,
});

function showWorkspaceView(): void {
  navigation.activeView = 'workspace';
}

/**
 * Persist the current navigation target.
 * Saves `null` for the home screen or the project ID.
 */
function persistLastProject(projectId: string | null): void {
  setStoreValue(LAST_PROJECT_STORE_KEY, projectId);
}

/**
 * Restore the last viewed project on app launch.
 *
 * Must be called after `initPersistentStore()` (which is done inside
 * `initPreferences()`). If the stored project no longer exists the
 * user is sent to the home screen instead.
 */
export async function initNavigation(): Promise<void> {
  const lastProjectId = await getStoreValue<string | null>(LAST_PROJECT_STORE_KEY);
  if (!lastProjectId) return;

  // Validate the project still exists before navigating to it
  try {
    const projects = await commands.listProjects();
    projectsList.current = projects;
    const existingIds = new Set(projects.map((p) => p.id));
    if (existingIds.has(lastProjectId)) {
      navigation.selectedProjectId = lastProjectId;
    } else {
      // Project was deleted — clear the stale value
      await setStoreValue(LAST_PROJECT_STORE_KEY, null);
    }
    // Remove unread entries for projects that no longer exist
    await projectStateStore.pruneDeletedProjects(existingIds);
  } catch {
    // If we can't list projects (e.g. store error), stay on home
    console.warn('[Navigation] Could not verify last project, falling back to home');
  }
}

/** Navigate to the repos list view. */
export function showAllRepos(): void {
  if (!reposUiEnabled) return;
  showWorkspaceView();
  navigation.selectedProjectId = null;
  navigation.showReposList = true;
}

/**
 * The most recent project switch, used to measure how long the detail page
 * takes to render. Each switch gets a fresh, monotonically increasing token so
 * downstream debug effects can tell whether they are reporting the initial
 * post-switch render or a later re-fire (which would otherwise report a
 * misleading elapsed-since-switch). See [perf][project-switch] debug logs in
 * ProjectHome/ProjectSection.
 */
let currentSwitch = { projectId: null as string | null, at: 0, token: 0 };

/** Elapsed ms since the most recent project switch, for debug perf logging. */
export function msSinceProjectSwitch(): number {
  return performance.now() - currentSwitch.at;
}

/** Token identifying the most recent project switch (bumped on every switch). */
export function currentProjectSwitchToken(): number {
  return currentSwitch.token;
}

/** The project id targeted by the most recent switch, or null. */
export function currentProjectSwitchTarget(): string | null {
  return currentSwitch.projectId;
}

/** Navigate to a specific project's detail view. */
export function selectProject(projectId: string): void {
  const startedAt = performance.now();
  currentSwitch = { projectId, at: startedAt, token: currentSwitch.token + 1 };
  console.info(
    `[perf][project-switch] selectProject('${projectId}') — switch started (token ${currentSwitch.token})`
  );
  showWorkspaceView();
  navigation.selectedProjectId = projectId;
  navigation.showReposList = false;
  persistLastProject(projectId);
  // Mark the project as read when navigating to it, but only if it's not already read
  if (projectStateStore.isUnread(projectId)) {
    projectStateStore.markAsRead(projectId);
  }
  // Stamp the end of the synchronous body so we can isolate how much of the
  // switch latency is spent here vs. in the (unmeasured) keyed-block swap that
  // tears down the old ProjectSection and mounts the new one.
  console.info(
    `[perf][project-switch] selectProject('${projectId}') — synchronous body done in ` +
      `${(performance.now() - startedAt).toFixed(1)}ms (token ${currentSwitch.token})`
  );
}

/** Navigate to a project and scroll to a specific branch card. */
export function selectProjectAndBranch(projectId: string, branchId: string): void {
  showWorkspaceView();
  const alreadyOnProject = navigation.selectedProjectId === projectId;
  navigation.selectedProjectId = projectId;
  navigation.showReposList = false;
  persistLastProject(projectId);
  // Mark the project as read when navigating to it, but only if it's not already read
  if (projectStateStore.isUnread(projectId)) {
    projectStateStore.markAsRead(projectId);
  }
  if (alreadyOnProject) {
    // Already mounted, scroll immediately.
    window.dispatchEvent(new CustomEvent('staged:scroll-to-branch', { detail: { branchId } }));
  } else {
    // Allow ProjectHome to mount and register its scroll-to-branch listener.
    setTimeout(() => {
      window.dispatchEvent(new CustomEvent('staged:scroll-to-branch', { detail: { branchId } }));
    }, 150);
  }
}

/** Returns true when a modal or diff overlay is open. */
function isModalOpen(): boolean {
  return !!document.querySelector('[role="dialog"], .diff-modal-backdrop');
}

/** Navigate to the previous project in the list. */
export function selectPreviousProject(): void {
  if (!navigation.selectedProjectId || navigation.activeView !== 'workspace' || isModalOpen())
    return;
  const projects = projectsList.current;
  const currentIndex = projects.findIndex((p) => p.id === navigation.selectedProjectId);
  if (currentIndex > 0) {
    selectProject(projects[currentIndex - 1].id);
  }
}

/** Navigate to the next project in the list. */
export function selectNextProject(): void {
  if (!navigation.selectedProjectId || navigation.activeView !== 'workspace' || isModalOpen())
    return;
  const projects = projectsList.current;
  const currentIndex = projects.findIndex((p) => p.id === navigation.selectedProjectId);
  if (currentIndex >= 0 && currentIndex < projects.length - 1) {
    selectProject(projects[currentIndex + 1].id);
  }
}

/** Navigate back to the projects list (landing page). */
export function goHome(): void {
  showWorkspaceView();
  requestProjectsListRestore(navigation.selectedProjectId);
  navigation.selectedProjectId = null;
  navigation.showReposList = false;
  persistLastProject(null);
}

/** Show the dedicated settings view and select a settings section. */
export function openSettings(section: SettingsSection = 'general'): void {
  navigation.settingsSection = section;
  navigation.activeView = 'settings';
}

/** Return from settings to the workspace view (project or home). */
export function closeSettings(): void {
  showWorkspaceView();
}
