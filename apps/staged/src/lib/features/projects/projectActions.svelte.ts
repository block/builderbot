/**
 * Shared project context-menu actions: set status, mark unread and remove
 * project.
 *
 * ProjectHome (top-bar button + ⌘⌫ shortcut), ProjectsList (card context
 * menu), and ProjectsSidebar (row context menu, rendered from App.svelte on
 * both the project and repos routes) all expose mark unread and remove. Set
 * status backs both context menus' Set Status submenu and the detail page's
 * status picker. This module owns the orchestration — the optimistic status
 * write, the safe-to-delete check, the pending-confirmation state behind the
 * shared dialog (ProjectDeleteDialog, mounted once in App.svelte), and the
 * delete lifecycle against the projectsData store — so the flow behaves
 * identically on every route instead of each view carrying its own copy.
 */

import { tick } from 'svelte';
import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
import type { Project } from '../../types';
import { projectDisplayName } from '../../shared/utils';
import {
  goHome,
  navigation,
  releaseProjectLayer,
  selectProject,
} from '../layout/navigation.svelte';
import { projectsDataStore } from '../../stores/projectsData.svelte';
import { projectStateStore } from '../../stores/projectState.svelte';
import { canDeleteProjectWithoutConfirmation } from './projectDeleteSafety';
import { focusIfDropped, type FocusAfterRemove } from './focusAfterRemove';
import { workspaceLifecycle } from './workspaceLifecycle.svelte';

class ProjectActionsController {
  /** Project awaiting the user's answer in the shared confirmation dialog. */
  private _pendingDelete = $state<Project | null>(null);
  /** Where focus goes if the pending delete is confirmed, from its list. */
  private pendingFocusAfterRemove: FocusAfterRemove | null = null;
  /** The confirmed delete's, until the dialog's close takes it. */
  private confirmedFocusAfterRemove: FocusAfterRemove | null = null;

  /** Each project's queue tail resolves to its last successfully persisted status. */
  private readonly statusOverrideWrites = new Map<
    string,
    { statusOverride: string | null; settled: Promise<string | null> }
  >();

  get pendingDelete(): Project | null {
    return this._pendingDelete;
  }

  markProjectUnread(project: Project): void {
    if (projectsDataStore.isProjectDeleting(project.id)) return;
    projectStateStore.markAsUnread(project.id);
  }

  /**
   * Choose a status option for a project, or null to go back to Default (the
   * computed PR status). Patches the project store at once so every surface
   * repaints. Writes run in selection order for each project, and a failed
   * write rolls back to the last confirmed status only if no newer selection
   * has superseded it.
   */
  async setProjectStatusOverride(project: Project, statusOverride: string | null): Promise<void> {
    if (projectsDataStore.isProjectDeleting(project.id)) return;
    const projectId = project.id;
    const pending = this.statusOverrideWrites.get(projectId);
    // A dangling id already reads as Default; choosing Default clears it too.
    // A refetch may show an earlier status while newer choices are queued.
    if (statusOverride === (pending ? pending.statusOverride : project.statusOverride)) return;

    const previous = pending?.settled ?? Promise.resolve(project.statusOverride);
    const settled: Promise<string | null> = previous.then(async (confirmedStatus) => {
      try {
        await commands.setProjectStatusOverride(projectId, statusOverride);
        return statusOverride;
      } catch (e) {
        if (this.statusOverrideWrites.get(projectId)?.settled === settled) {
          projectsDataStore.projectStatusOverrideChanged(projectId, confirmedStatus);
        }
        toast.error('Unable to set project status', {
          description: e instanceof Error ? e.message : String(e),
        });
        // Let later choices proceed, retaining a truthful rollback target.
        return confirmedStatus;
      }
    });
    this.statusOverrideWrites.set(projectId, { statusOverride, settled });
    projectsDataStore.projectStatusOverrideChanged(projectId, statusOverride);
    try {
      await settled;
    } finally {
      if (this.statusOverrideWrites.get(projectId)?.settled === settled) {
        this.statusOverrideWrites.delete(projectId);
      }
    }
  }

  /**
   * Remove a project: immediately when every branch is merged with nothing
   * unpushed, otherwise via the shared confirmation dialog. Resolves once the
   * delete finished or the dialog is up.
   *
   * A list passes `focusAfterRemove` when the remove comes from its card or
   * row, which the delete unmounts along with the focus it would return to.
   */
  async requestRemoveProject(project: Project, focusAfterRemove?: FocusAfterRemove): Promise<void> {
    if (projectsDataStore.isProjectDeleting(project.id)) return;

    // Rows render as soon as the project *list* lands, so branches and repos
    // may still be the seeded fallbacks (empty branch list; repoCount 1 for a
    // githubRepo project, 0 otherwise) when the user picks Remove. Deciding
    // against those would skip the confirmation dialog for an un-hydrated
    // multi-repo project — repoCount 0 reads as "nothing to lose" — and leave
    // branchesToClear empty, so demand the data instead of reading whatever
    // the cache happens to hold. Deduped: no refetch when already hydrated,
    // and joins the idle drip's request when one is in flight.
    await projectsDataStore.ensureProjectHydrated(project.id);
    if (projectsDataStore.isProjectDeleting(project.id)) return;

    // Hydration settles even when the fetch fails, so views gated on it don't
    // hang — but then branches and repoCount are still the fallbacks. Only the
    // success path writes reposByProject, so it is the truthful "actually
    // fetched" signal: without it, fall through to the dialog.
    const reposFetched = projectsDataStore.reposByProject.has(project.id);
    const safeToDelete =
      reposFetched &&
      (await canDeleteProjectWithoutConfirmation({
        branches: projectsDataStore.branchesByProject.get(project.id) || [],
        repoCount: projectsDataStore.repoCountsByProject.get(project.id) ?? 0,
        hasUnpushedCommits: commands.hasUnpushedCommits,
        onCheckError: (e) => console.error('Failed to check unpushed commits:', e),
      }));

    if (safeToDelete) {
      await this.deleteProject(project, focusAfterRemove);
    } else {
      this._pendingDelete = project;
      this.pendingFocusAfterRemove = focusAfterRemove ?? null;
      this.confirmedFocusAfterRemove = null;
    }
  }

  /** Confirm the pending delete (dialog "Remove" button). */
  async confirmPendingDelete(): Promise<void> {
    const project = this._pendingDelete;
    const focusAfterRemove = this.pendingFocusAfterRemove;
    this._pendingDelete = null;
    this.pendingFocusAfterRemove = null;
    if (project) {
      this.confirmedFocusAfterRemove = focusAfterRemove;
      await this.deleteProject(project, focusAfterRemove ?? undefined);
    }
  }

  /** Dismiss the confirmation dialog without deleting. */
  cancelPendingDelete(): void {
    this._pendingDelete = null;
    this.pendingFocusAfterRemove = null;
  }

  /**
   * For the confirmation dialog's close-focus handler: the element to focus
   * after a confirmed remove, or null to leave focus to the dialog, as after
   * a cancel. Taken once. The dialog still holds focus when the project is
   * hidden, so it hands focus over itself rather than wait for it to drop.
   */
  takeFocusAfterRemove(): HTMLElement | null {
    const resolve = this.confirmedFocusAfterRemove;
    this.confirmedFocusAfterRemove = null;
    return resolve ? resolve() : null;
  }

  private async deleteProject(
    project: Project,
    focusAfterRemove?: FocusAfterRemove
  ): Promise<void> {
    if (projectsDataStore.isProjectDeleting(project.id)) return;

    const id = project.id;
    const name = projectDisplayName(project);
    const branchesToClear = projectsDataStore.branchesByProject.get(id) || [];
    // Marks the project deleting, which also hides it from the grid, the
    // sidebar and the pickers (projectsDataStore.liveProjects) right away.
    projectsDataStore.projectDeleteStarted(id, name);

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

    // Drop the project's layer as soon as the delete starts, so it unmounts
    // now rather than sit clipped under home showing "Deleting…" until the
    // backend echo prunes the project. releaseProjectLayer only acts while
    // home is active, so this runs after the navigation above: a project
    // retained under home is released as is, the on-screen project whose
    // goHome() just made home active is released too, and after
    // selectProject(next) the layer already names `next`, so it is a no-op.
    releaseProjectLayer(id);

    // Its card or row, and its context menu with it, unmounts in the next
    // flush. Focus that was in either drops to <body> then. Focus still in
    // the confirmation dialog is handed over by the dialog's close instead
    // (takeFocusAfterRemove).
    if (focusAfterRemove) void tick().then(() => focusIfDropped(focusAfterRemove));

    try {
      await commands.deleteProject(id);
      // The delete's project-changed event prunes the project and its
      // deleting marker from projectsDataStore, and each branch's
      // branch-changed invalidates its timeline caches. In web mode an echo
      // lost to a WebSocket gap is recovered by the reconnect revalidation in
      // transport.ts, so the marker can't outlive the socket outage.
      projectStateStore.markAsRead(id);
      for (const branch of branchesToClear) {
        workspaceLifecycle.clearBranchState(branch.id);
      }
      // The card left at confirm without a word; this is what says the
      // remove went through, to a screen reader as much as on screen.
      toast.success(`Removed "${name}"`);
    } catch (e) {
      console.error('Failed to delete project:', e);
      const message = e instanceof Error ? e.message : String(e);
      toast.error('Unable to delete project', { description: message });
      // Clearing the marker brings the hidden card back next to the toast.
      projectsDataStore.projectDeleteFailed(id);
    }
  }
}

export const projectActions = new ProjectActionsController();
