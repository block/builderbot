/**
 * Shared project context-menu actions: mark unread, transfer and remove
 * project. (Tag writes, including the context menu's tag submenus, live in
 * features/tags/tagActions.)
 *
 * ProjectHome (top-bar button + ⌘⌫ shortcut), ProjectsList (card context
 * menu), and ProjectsSidebar (row context menu, rendered from App.svelte on
 * both the project and repos routes) all expose mark unread and remove. This
 * module owns the orchestration — the safe-to-delete check, the
 * pending-confirmation state behind the shared dialog (ProjectDeleteDialog,
 * mounted once in App.svelte), and the delete lifecycle against the
 * projectsData store — so the flow behaves identically on every route
 * instead of each view carrying its own copy.
 * Transfer follows the same shape: the preflight behind the shared
 * ProjectTransferDialog, then the transfer itself, whose progress the
 * projectsData store follows from the backend's events.
 */

import { tick } from 'svelte';
import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
import type {
  Project,
  ProjectTransferDirection,
  ProjectTransferPreflight,
  ProjectTransferPreflightEvent,
} from '../../types';
import { listenToEvent } from '../../transport';
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
import {
  setTransferIgnoredFiles,
  setTransferUncommittedChanges,
} from '../settings/preferences.svelte';
import {
  asksAboutIgnored,
  asksAboutUncommitted,
  ignoredTransferBlocked,
  isTransferCancelledError,
  isTransferStaleError,
  issueMessage,
  placeholderPreflight,
  transferDestinationLabel,
  transferDirectionFrom,
  transferResultWarnings,
} from './projectTransfer';

/** The project behind the shared transfer dialog, and its preflight's state. */
export type PendingTransfer = { project: Project; direction: ProjectTransferDirection } & (
  | { status: 'loading' }
  | { status: 'ready'; preflight: ProjectTransferPreflight }
  | { status: 'error'; error: string }
);

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

class ProjectActionsController {
  /** Project awaiting the user's answer in the shared confirmation dialog. */
  private _pendingDelete = $state<Project | null>(null);
  /** Where focus goes if the pending delete is confirmed, from its list. */
  private pendingFocusAfterRemove: FocusAfterRemove | null = null;
  /** The confirmed delete's, bound to what it opened instead, until the
   *  dialog's close takes it. */
  private confirmedFocusAfterRemove: (() => HTMLElement | null) | null = null;

  /** Project awaiting the user's answer in the shared transfer dialog. */
  private _pendingTransfer = $state<PendingTransfer | null>(null);
  /** Bumped per dialog open and close, so a late preflight can't land on a
   *  dialog that was closed or reopened for another project. */
  private transferRequest = 0;
  /** The streaming preflight scan behind the open transfer dialog, if any. */
  private activeScan: { scanId: string; finish: () => void } | null = null;

  get pendingDelete(): Project | null {
    return this._pendingDelete;
  }

  get pendingTransfer(): PendingTransfer | null {
    return this._pendingTransfer;
  }

  markProjectUnread(project: Project): void {
    if (projectsDataStore.isProjectDeleting(project.id)) return;
    projectStateStore.markAsUnread(project.id);
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
    // The backend refuses to delete a project mid-transfer.
    if (projectsDataStore.isProjectTransferring(project.id)) return;

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
    if (project) await this.deleteProject(project, focusAfterRemove ?? undefined, true);
  }

  /** Dismiss the confirmation dialog without deleting. */
  cancelPendingDelete(): void {
    this._pendingDelete = null;
    this.pendingFocusAfterRemove = null;
  }

  /**
   * Open the transfer dialog for a project and scan its preflight: what each
   * branch would carry and what stops the transfer. The dialog opens with
   * the branches already known here, each "Checking…", and the Transfer
   * button live from the first frame (confirming mid-scan asks first, since
   * files not yet listed may be lost). The scan streams into it: the
   * backend's store view first, then what Blox says of the workspaces, then
   * each row's counts as its checkout is read. Resolves once the scan has
   * finished, failed or been stopped by a confirm or cancel.
   */
  async requestTransferProject(project: Project): Promise<void> {
    if (
      projectsDataStore.isProjectDeleting(project.id) ||
      projectsDataStore.isProjectTransferring(project.id)
    ) {
      return;
    }
    const request = ++this.transferRequest;
    this.stopActiveScan();
    const direction = transferDirectionFrom(project.location);
    // The branches this store knows are enough to open with; the scan's
    // steps replace them. A project whose branches haven't loaded yet opens
    // loading instead, for the few milliseconds until the scan's first step.
    const placeholder = placeholderPreflight(
      project,
      projectsDataStore.branchesByProject.get(project.id) ?? [],
      projectsDataStore.reposByProject.get(project.id) ?? []
    );
    this._pendingTransfer = placeholder
      ? { project, direction, status: 'ready', preflight: placeholder }
      : { project, direction, status: 'loading' };
    // The progress banner names repos from the project's own, so fetch them
    // meanwhile (deduped like the remove flow's). Never rejects, and the
    // dialog doesn't wait.
    void projectsDataStore.ensureProjectHydrated(project.id);

    const scanId = crypto.randomUUID();
    const current = () => request === this.transferRequest;
    await new Promise<void>((resolve) => {
      let finished = false;
      let unlisten: (() => void) | null = null;
      const finish = () => {
        if (finished) return;
        finished = true;
        if (this.activeScan?.scanId === scanId) this.activeScan = null;
        unlisten?.();
        resolve();
      };
      const fail = (error: unknown) => {
        if (current()) {
          this._pendingTransfer = {
            project,
            direction,
            status: 'error',
            error: errorMessage(error),
          };
        }
        finish();
      };
      unlisten = listenToEvent<ProjectTransferPreflightEvent>(
        'project-transfer-preflight',
        (event) => {
          if (event.scanId !== scanId) return;
          if (!current()) {
            finish();
            return;
          }
          if (event.preflight) {
            this._pendingTransfer = {
              project,
              direction: event.preflight.direction,
              status: 'ready',
              preflight: event.preflight,
            };
          }
          if (!event.done) return;
          // A cancelled scan was stopped by this controller (confirm or
          // cancel), so the dialog's state is already settled.
          if (event.error && !event.cancelled) {
            this._pendingTransfer = { project, direction, status: 'error', error: event.error };
          }
          finish();
        },
        {
          // Start only once the listener is in place, so the first partial
          // (or a quick failure) can't slip past it.
          onEstablished: () => {
            if (!current()) {
              finish();
              return;
            }
            this.activeScan = { scanId, finish };
            commands.startProjectTransferPreflight(project.id, scanId).catch(fail);
          },
          onRegistrationFailed: fail,
        }
      );
    });
  }

  /** Stop the dialog's streaming scan, if one is still going. */
  private stopActiveScan(): void {
    const scan = this.activeScan;
    if (!scan) return;
    this.activeScan = null;
    // Settles requestTransferProject: the backend's final event for a
    // stopped scan has nothing left to tell the dialog.
    scan.finish();
    commands.cancelProjectTransferPreflight(scan.scanId).catch((e) => {
      console.error('Failed to cancel the transfer preflight scan:', e);
    });
  }

  /**
   * Start the pending transfer (dialog "Transfer" button). The dialog closes
   * at once; progress shows on the project and the outcome as a toast.
   * `includeUncommitted` is the checkbox, which only shows (and is only
   * remembered) when some repo has, or may have, uncommitted changes (see
   * asksAboutUncommitted). Without it there is nothing to opt out of, so
   * whatever turns up meanwhile is carried.
   *
   * `includeIgnored` is the second checkbox, which shows only when some repo
   * has ignored files (asksAboutIgnored) and can't be ticked when they are
   * over the cap (ignoredTransferBlocked). It is remembered, and sent, only
   * when it was shown and could be ticked; otherwise ignored files stay in
   * the backup, as they always did.
   */
  async confirmPendingTransfer({
    includeUncommitted,
    includeIgnored,
  }: {
    includeUncommitted: boolean;
    includeIgnored: boolean;
  }): Promise<void> {
    const pending = this._pendingTransfer;
    if (pending?.status !== 'ready' || !pending.preflight.canTransfer) return;
    const { project, preflight } = pending;
    this._pendingTransfer = null;
    this.transferRequest += 1;
    // A scan still going is stopped rather than finished: the transfer reads
    // the checkouts again itself, and the user has already accepted that
    // files not yet listed may be lost.
    this.stopActiveScan();
    const asked = asksAboutUncommitted(preflight);
    if (asked) {
      setTransferUncommittedChanges(includeUncommitted);
    }
    const askedIgnored = asksAboutIgnored(preflight) && ignoredTransferBlocked(preflight) === null;
    if (askedIgnored) {
      setTransferIgnoredFiles(includeIgnored);
    }
    await this.transferProject(project, preflight, {
      includeUncommitted: asked ? includeUncommitted : true,
      includeIgnored: askedIgnored && includeIgnored,
    });
  }

  /** Dismiss the transfer dialog without transferring. */
  cancelPendingTransfer(): void {
    this._pendingTransfer = null;
    this.transferRequest += 1;
    this.stopActiveScan();
  }

  /**
   * Ask a running transfer to stop before it switches the project over (the
   * progress banner's Cancel). Resolves whether one was running; the
   * transfer's own promise reports the outcome.
   */
  async cancelTransfer(projectId: string): Promise<boolean> {
    try {
      return await commands.cancelProjectTransfer(projectId);
    } catch (e) {
      toast.error('Unable to cancel the transfer', { description: errorMessage(e) });
      return false;
    }
  }

  private async transferProject(
    project: Project,
    preflight: ProjectTransferPreflight,
    { includeUncommitted, includeIgnored }: { includeUncommitted: boolean; includeIgnored: boolean }
  ): Promise<void> {
    const id = project.id;
    if (projectsDataStore.isProjectDeleting(id) || projectsDataStore.isProjectTransferring(id)) {
      return;
    }
    const name = projectDisplayName(project);
    const direction = preflight.direction;
    const destination = transferDestinationLabel(direction);
    projectsDataStore.projectTransferStarted(id, direction);
    try {
      const result = await commands.transferProject(
        id,
        direction,
        includeUncommitted,
        includeIgnored
      );
      projectsDataStore.projectTransferFinished(id, result.transferId);
      const warnings = transferResultWarnings(result, preflight);
      const notes = warnings.map(issueMessage);
      const leftBehind = result.repos.some((repo) => repo.uncommittedLeftBehind);
      if (leftBehind) {
        notes.push(
          result.backupDir
            ? `Uncommitted changes were left behind in the backup at ${result.backupDir}.`
            : 'Uncommitted changes were left behind in the backup.'
        );
      }
      const title = `Transferred "${name}" to ${destination}`;
      const description = notes.length > 0 ? notes.join(' ') : undefined;
      if (warnings.length > 0) {
        toast.warning(title, { description, duration: Infinity });
      } else {
        // The backup path is how the user gets those changes back, so it
        // stays until dismissed.
        toast.success(title, { description, ...(leftBehind ? { duration: Infinity } : {}) });
      }
    } catch (e) {
      const message = errorMessage(e);
      projectsDataStore.projectTransferFinished(id);
      if (isTransferCancelledError(message)) {
        toast.info(`Cancelled transferring "${name}"`, {
          description:
            direction === 'toRemote'
              ? 'Nothing changed; the project is still local.'
              : 'Nothing changed; the project is still on Blox.',
        });
      } else if (isTransferStaleError(message)) {
        // The dialog was confirmed after another window moved the project:
        // its destination is where the project already is, so say what the
        // backend found rather than name it, and fetch where the project is
        // now, so a new dialog offers the way it does go.
        toast.warning(`Didn't transfer "${name}"`, { description: message });
        void projectsDataStore.refreshTransferredProject(id);
      } else {
        console.error('Failed to transfer project:', e);
        toast.error(`Unable to transfer "${name}" to ${destination}`, {
          description: message,
          duration: Infinity,
        });
      }
    }
  }

  /**
   * For the confirmation dialog's close-focus handler: the element to focus
   * after a confirmed remove, or null to leave focus to the dialog, as after
   * a cancel. Taken once. Focus stays in the dialog when the project is
   * hidden (see deleteProject), so the dialog hands it over itself rather
   * than wait for it to drop.
   */
  takeFocusAfterRemove(): HTMLElement | null {
    const resolve = this.confirmedFocusAfterRemove;
    this.confirmedFocusAfterRemove = null;
    return resolve ? resolve() : null;
  }

  /** `fromDialog`: confirmed in the dialog, whose close hands focus over. */
  private async deleteProject(
    project: Project,
    focusAfterRemove?: FocusAfterRemove,
    fromDialog = false
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
    let openedInstead: string | null = null;
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
        openedInstead = next.id;
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
    // flush, and focus that was in either drops to <body>. focusIfDropped
    // picks it up once that flush has run.
    //
    // A confirmed dialog's Remove button unmounts in the same flush, as the
    // dialog's footer only renders while a delete is pending. Focus doesn't
    // drop then: bits-ui's FocusScope sees the button it last focused go and
    // focuses the dialog's content, from a mutation-observer microtask that
    // runs before this check. focusIfDropped leaves that alone, and the
    // dialog's close hands focus over instead (takeFocusAfterRemove). Were
    // that order to flip, focusIfDropped would focus the pick first, the
    // still-open dialog's trap would pull focus back, and the close would
    // land it on the same pick, as both resolve it the same way.
    if (focusAfterRemove) {
      const resolveFocus = () => focusAfterRemove(openedInstead);
      if (fromDialog) this.confirmedFocusAfterRemove = resolveFocus;
      void tick().then(() => focusIfDropped(resolveFocus));
    }

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
