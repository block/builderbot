/**
 * Shared reactive state for projects and branches.
 *
 * Singleton module — both the Sidebar and ProjectHome consume this
 * so data is loaded once and stays in sync across components.
 */

import type { Project, Branch, StoreIncompatibility } from '../../types';
import * as commands from '../../commands';

// =============================================================================
// Reactive State
// =============================================================================

export const projectStore = $state({
  projects: [] as Project[],
  branchesByProject: new Map<string, Branch[]>(),
  loading: true,
  error: null as string | null,

  /** Non-null when the DB needs a reset or is too new. */
  storeIncompat: null as StoreIncompatibility | null,
  resetting: false,

  /** Branch IDs currently being deleted (for UI "Deleting…" state). */
  deletingBranches: new Set<string>(),
});

// =============================================================================
// Data Loading
// =============================================================================

/** Check store health, then load projects + branches. */
export async function checkStoreAndLoad(): Promise<void> {
  projectStore.loading = true;
  try {
    const status = await commands.getStoreStatus();
    if (status) {
      projectStore.storeIncompat = status;
      projectStore.loading = false;
      return;
    }
    await loadData();
  } catch (e) {
    projectStore.error = e instanceof Error ? e.message : String(e);
    projectStore.loading = false;
  }
}

/** Reset the store after user confirmation, then reload. */
export async function handleResetStore(): Promise<void> {
  projectStore.resetting = true;
  try {
    await commands.confirmResetStore();
    projectStore.storeIncompat = null;
    await loadData();
  } catch (e) {
    projectStore.error = e instanceof Error ? e.message : String(e);
  } finally {
    projectStore.resetting = false;
  }
}

/** Load all projects and their branches from the backend. */
export async function loadData(): Promise<void> {
  projectStore.loading = true;
  projectStore.error = null;
  try {
    const projectList = await commands.listProjects();
    projectStore.projects = projectList;

    const branchMap = new Map<string, Branch[]>();
    await Promise.all(
      projectList.map(async (project) => {
        const branches = await commands.listBranchesForProject(project.id);
        branchMap.set(project.id, branches);
      })
    );
    projectStore.branchesByProject = branchMap;
  } catch (e) {
    projectStore.error = e instanceof Error ? e.message : String(e);
  } finally {
    projectStore.loading = false;
  }
}

// =============================================================================
// Project Mutations
// =============================================================================

/** Add a newly created project and load its branches. */
export async function addProject(project: Project): Promise<void> {
  if (!projectStore.projects.some((p) => p.id === project.id)) {
    projectStore.projects = [...projectStore.projects, project];
  }
  const branches = await commands.listBranchesForProject(project.id);
  projectStore.branchesByProject = new Map(projectStore.branchesByProject).set(
    project.id,
    branches
  );
}

/** Delete a project and remove it from state. */
export async function removeProject(id: string): Promise<void> {
  await commands.deleteProject(id);
  projectStore.projects = projectStore.projects.filter((p) => p.id !== id);
  const newMap = new Map(projectStore.branchesByProject);
  newMap.delete(id);
  projectStore.branchesByProject = newMap;
}

// =============================================================================
// Branch Mutations
// =============================================================================

/** Add a newly created branch to its project's list. */
export function addBranch(branch: Branch): void {
  const existing = projectStore.branchesByProject.get(branch.projectId) || [];
  projectStore.branchesByProject = new Map(projectStore.branchesByProject).set(branch.projectId, [
    ...existing,
    branch,
  ]);
}

/** Replace a branch record (e.g. after worktree setup populates worktreePath). */
export function updateBranch(updated: Branch): void {
  const branches = projectStore.branchesByProject.get(updated.projectId) || [];
  projectStore.branchesByProject = new Map(projectStore.branchesByProject).set(
    updated.projectId,
    branches.map((b) => (b.id === updated.id ? updated : b))
  );
}

/** Remove a branch after successful deletion. */
export function removeBranch(branch: Branch): void {
  const existing = projectStore.branchesByProject.get(branch.projectId) || [];
  projectStore.branchesByProject = new Map(projectStore.branchesByProject).set(
    branch.projectId,
    existing.filter((b) => b.id !== branch.id)
  );
}

/** Mark a branch as "deleting" (UI optimistic state). */
export function setDeletingBranch(branchId: string): void {
  projectStore.deletingBranches = new Set([...projectStore.deletingBranches, branchId]);
}

/** Clear the "deleting" state for a branch. */
export function clearDeletingBranch(branchId: string): void {
  const next = new Set(projectStore.deletingBranches);
  next.delete(branchId);
  projectStore.deletingBranches = next;
}
