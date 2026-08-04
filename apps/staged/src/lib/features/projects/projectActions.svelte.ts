/**
 * Shared project context-menu actions: mark unread and remove project.
 *
 * ProjectHome (top-bar button + ⌘⌫ shortcut), ProjectsList (card context
 * menu), and ProjectsSidebar (row context menu, rendered from App.svelte on
 * both the project and repos routes) all expose the same two actions. This
 * module owns the orchestration — the safe-to-delete check, the
 * pending-confirmation state behind the shared dialog (ProjectDeleteDialog,
 * mounted once in App.svelte), and the delete lifecycle against the
 * projectsData store — so the flow behaves identically on every route
 * instead of each view carrying its own copy.
 */

import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
import type { Project } from '../../types';
import { projectDisplayName } from '../../shared/utils';
import { goHome, navigation, selectProject } from '../layout/navigation.svelte';
import { projectsDataStore } from '../../stores/projectsData.svelte';
import { projectStateStore } from '../../stores/projectState.svelte';
import { canDeleteProjectWithoutConfirmation } from './projectDeleteSafety';
import { workspaceLifecycle } from './workspaceLifecycle.svelte';

class ProjectActionsController {
  /** Project awaiting the user's answer in the shared confirmation dialog. */
  private _pendingDelete = $state<Project | null>(null);

  get pendingDelete(): Project | null {
    return this._pendingDelete;
  }

  markProjectUnread(project: Project): void {
    if (projectsDataStore.isProjectDeleting(project.id)) return;
    projectStateStore.markAsUnread(project.id);
  }

  /**
   * Remove a project: immediately when every branch is merged with nothing
   * unpushed, otherwise via the shared confirmation dialog. Resolves once the
   * delete finished or the dialog is up.
   */
  async requestRemoveProject(project: Project): Promise<void> {
    if (projectsDataStore.isProjectDeleting(project.id)) return;

    const safeToDelete = await canDeleteProjectWithoutConfirmation({
      branches: projectsDataStore.branchesByProject.get(project.id) || [],
      repoCount: projectsDataStore.repoCountsByProject.get(project.id) ?? 0,
      hasUnpushedCommits: commands.hasUnpushedCommits,
      onCheckError: (e) => console.error('Failed to check unpushed commits:', e),
    });

    if (safeToDelete) {
      await this.deleteProject(project);
    } else {
      this._pendingDelete = project;
    }
  }

  /** Confirm the pending delete (dialog "Remove" button). */
  async confirmPendingDelete(): Promise<void> {
    const project = this._pendingDelete;
    this._pendingDelete = null;
    if (project) {
      await this.deleteProject(project);
    }
  }

  /** Dismiss the confirmation dialog without deleting. */
  cancelPendingDelete(): void {
    this._pendingDelete = null;
  }

  private async deleteProject(project: Project): Promise<void> {
    if (projectsDataStore.isProjectDeleting(project.id)) return;

    const id = project.id;
    const branchesToClear = projectsDataStore.branchesByProject.get(id) || [];
    projectsDataStore.projectDeleteStarted(id, projectDisplayName(project));

    // When the project on screen is being deleted, navigate away immediately
    // so the user doesn't have to wait for backend deletion. Deleting any
    // other project (sidebar/landing-grid context menu) keeps the current view.
    if (navigation.selectedProjectId === id) {
      const projects = projectsDataStore.projects;
      const currentIndex = projects.findIndex((p) => p.id === id);
      const alive = projects.filter(
        (p) => p.id !== id && !projectsDataStore.isProjectDeleting(p.id)
      );
      if (alive.length > 0) {
        // Prefer the next project after the current one; fall back to the closest earlier one
        const next =
          alive.find((p) => projects.indexOf(p) > currentIndex) ?? alive[alive.length - 1];
        selectProject(next.id);
      } else {
        goHome();
      }
    }

    try {
      await commands.deleteProject(id);
      projectStateStore.markAsRead(id);
      projectsDataStore.projectDeleteFinished(id, { removed: true });
      commands.invalidateProjectBranchTimelines(branchesToClear.map((b) => b.id));
      for (const branch of branchesToClear) {
        workspaceLifecycle.clearBranchState(branch.id);
      }
    } catch (e) {
      console.error('Failed to delete project:', e);
      const message = e instanceof Error ? e.message : String(e);
      toast.error('Unable to delete project', { description: message });
      projectsDataStore.projectDeleteFinished(id);
    }
  }
}

export const projectActions = new ProjectActionsController();
