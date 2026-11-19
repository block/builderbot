import type { Branch, ProjectLocation, WorkspaceStatus } from '../../types';
import * as commands from '../../api/commands';
import { listenToEvent, type UnlistenFn } from '../../transport';
import { toast } from 'svelte-sonner';
import { getPreferredAgent } from '../settings/preferences.svelte';
import { agentState } from '../agents/agent.svelte';

type BranchMap = Map<string, Branch[]>;

interface LocalSetupTask {
  branchId: string;
  run: () => Promise<void>;
}

/** Payload of the backend's `workspace-setup-error` event: why a setup run
 *  left the branch in `error`, or null once it has no reason any more.
 *  `userRequested` marks the failure of a setup the user asked for (a new
 *  branch, Retry, Resume), as opposed to one the backend ran on its own.
 *  `notice` comes with a setup that worked and did something the user
 *  should know of: it put commits it found on the base branch on this one.
 *  With a `noticeTitle` it is about something else, under that heading: why
 *  the branch's queued session is waiting. */
interface WorkspaceSetupErrorEvent {
  branchId: string;
  error: string | null;
  userRequested?: boolean;
  notice?: string;
  noticeTitle?: string;
}

interface WorkspaceLifecycleHooks {
  getBranchesByProject: () => BranchMap;
  setBranchesByProject: (next: BranchMap) => void;
  isProjectDeleting: (projectId: string) => boolean;
  /** A transferring project's branches are the transfer's to provision: the
   *  backend refuses worktree and workspace setup until it is over. */
  isProjectTransferring: (projectId: string) => boolean;
  /** Where the project lives now; null when it isn't known. */
  getProjectLocation: (projectId: string) => ProjectLocation | null;
  /** Refetch a project a transfer of it just ended (its location and
   *  branches), settling once the fresh lists have landed. */
  refreshTransferredProject: (projectId: string) => Promise<void>;
}

class WorkspaceLifecycleController {
  private hooks: WorkspaceLifecycleHooks | null = null;

  private worktreeErrors = $state<Map<string, string>>(new Map());
  private workspaceErrors = $state<Map<string, string>>(new Map());
  private version = $state(0);

  private pendingSetupBranches = new Set<string>();
  private queuedSetupBranches = new Set<string>();
  /** Projects a transfer just ended for, skipped until their refetch lands:
   *  the lists held until then describe the old location. */
  private awaitingTransferRefresh = new Set<string>();
  /** Branches a transfer handed over, whose worktree (if they land without
   *  one) is set up with prerun actions, as a newly created branch's is. */
  private prerunOnSetup = new Set<string>();

  private activeLocalSetupCount = 0;
  private readonly MAX_LOCAL_SETUP_CONCURRENCY = 1;

  private localSetupTaskQueue: LocalSetupTask[] = [];

  // Single adaptive poller: 3s while any workspace is starting, 30s while
  // any is running, stopped otherwise. A remote branch's setup is the
  // backend's from start to finish: this controller asks for it, and the
  // poll and the `workspace-setup-error` event say how it went. It keeps no
  // clock of its own on a `starting` branch; the backend's waits are bounded
  // and end in `running` or `error` on the row.
  private readonly STARTING_POLL_MS = 3000;
  private readonly RUNNING_POLL_MS = 30_000;
  /** Bumped by start() and stop(), so an answer arriving after a restart
   *  does not write into the next run's state. */
  private generation = 0;
  private unlistenSetupErrors: UnlistenFn | null = null;
  /** Counts setup-error events, and the count at each branch's latest one,
   *  so a setup-error snapshot does not overwrite a newer event. */
  private setupErrorEventSeq = 0;
  private setupErrorEventSeqByBranch = new Map<string, number>();

  private pollTimer: ReturnType<typeof setTimeout> | null = null;
  private pollInFlight = false;
  private kickoffTimer: ReturnType<typeof setTimeout> | null = null;

  start(hooks: WorkspaceLifecycleHooks): void {
    this.hooks = hooks;
    const generation = ++this.generation;
    this.unlistenSetupErrors?.();
    // The backend reports every failed setup run, whoever asked for it (its
    // startup pass, poll-triggered runs and automatic retries included). The
    // snapshot covers failures from before this listener went live, and is
    // retaken on each web-socket reconnect.
    this.unlistenSetupErrors = listenToEvent<WorkspaceSetupErrorEvent>(
      'workspace-setup-error',
      (payload) => {
        if (this.generation !== generation) return;
        this.applySetupErrorEvent(payload);
      },
      { onEstablished: () => void this.seedSetupErrors(generation) }
    );
    this.ensurePolling();
  }

  stop(): void {
    this.generation++;
    this.unlistenSetupErrors?.();
    this.unlistenSetupErrors = null;
    this.setupErrorEventSeqByBranch.clear();
    this.stopPolling();
    if (this.kickoffTimer) {
      clearTimeout(this.kickoffTimer);
      this.kickoffTimer = null;
    }
    this.pendingSetupBranches.clear();
    this.queuedSetupBranches.clear();
    this.awaitingTransferRefresh.clear();
    this.prerunOnSetup.clear();
    this.localSetupTaskQueue = [];
    this.activeLocalSetupCount = 0;
    this.hooks = null;
  }

  getWorktreeErrors(): Map<string, string> {
    this.version;
    return this.worktreeErrors;
  }

  getWorkspaceErrors(): Map<string, string> {
    this.version;
    return this.workspaceErrors;
  }

  enqueueInitialSetup(projectId: string, branches: Branch[]): void {
    if (this.isProjectHeld(projectId)) return;
    let hasActiveRemote = false;
    for (const branch of branches) {
      this.enqueueBranchSetup(projectId, branch);
      if (
        branch.branchType === 'remote' &&
        (branch.workspaceStatus === 'starting' || branch.workspaceStatus === 'running')
      ) {
        hasActiveRemote = true;
      }
    }
    // Kick the poller so stale running/starting statuses get reconciled
    // against the Blox backend. Without this, branches loaded from the DB
    // with a stale "running" status would never be re-polled because the
    // poller was already stopped (no branches were loaded when start() ran).
    if (hasActiveRemote) {
      this.ensurePolling();
    }
  }

  scheduleKickoff(): void {
    if (this.kickoffTimer) clearTimeout(this.kickoffTimer);
    this.kickoffTimer = setTimeout(() => {
      this.kickoffTimer = null;
      this.kickOffPendingBranchSetup();
    }, 50);
  }

  handleWorkspaceStatusChange(
    projectId: string,
    branchId: string,
    workspaceStatus: WorkspaceStatus,
    workstationId?: number | null
  ): void {
    const hooks = this.hooks;
    if (!hooks) return;

    const current = hooks.getBranchesByProject();
    const branches = current.get(projectId);
    if (!branches) return;

    let changed = false;
    let wasError = false;
    const nextBranches = branches.map((branch) => {
      if (branch.id !== branchId) return branch;
      wasError = branch.workspaceStatus === 'error';
      const statusChanged = branch.workspaceStatus !== workspaceStatus;
      const idChanged = workstationId != null && branch.workstationId !== workstationId;
      if (!statusChanged && !idChanged) return branch;
      changed = true;
      return {
        ...branch,
        workspaceStatus,
        ...(workstationId != null ? { workstationId } : {}),
      };
    });

    if (changed) {
      hooks.setBranchesByProject(new Map(current).set(projectId, nextBranches));
    }

    // The reason goes once the branch is out of `error` or set up. Not on any
    // non-error status: a reason the error event delivered can precede the
    // poll that reports the branch's `error`, behind a stale `starting`.
    if (workspaceStatus === 'running' || (wasError && workspaceStatus !== 'error')) {
      this.clearWorkspaceError(branchId);
    }

    // Kick the poller when a workspace enters an active state. Queued
    // sessions are not drained from here: the backend kicks the branch queue
    // itself when it writes the `running` status.
    if (workspaceStatus === 'starting' || workspaceStatus === 'running') {
      this.ensurePolling();
    }
  }

  async retryWorktree(branchId: string, projectId: string): Promise<void> {
    await this.setupBranchWorktree(branchId, projectId, { runPrerun: true });
  }

  /**
   * Set a remote branch up again, typically one in `error`: the backend puts
   * it back to `starting` and reruns setup. A no-op while a request for the
   * branch is already out.
   */
  retryWorkspace(projectId: string, branchId: string): void {
    const hooks = this.hooks;
    if (!hooks || this.isProjectHeld(projectId)) return;
    if (this.pendingSetupBranches.has(branchId)) return;
    const branch = hooks
      .getBranchesByProject()
      .get(projectId)
      ?.find((b) => b.id === branchId);
    if (!branch || branch.branchType !== 'remote') return;
    // As in enqueueBranchSetup: a list read before a transfer switched the
    // project over would set up the old location again.
    const location = hooks.getProjectLocation(projectId);
    if (location && location !== 'remote') return;

    this.clearWorkspaceError(branchId);
    this.handleWorkspaceStatusChange(projectId, branchId, 'starting');
    this.requestRemoteSetup(branchId);
  }

  async resumeWorkspace(projectId: string, workspaceName: string): Promise<void> {
    // Optimistically transition all peer branches sharing this workspace.
    const hooks = this.hooks;
    const peerBranchIds: string[] = [];
    if (hooks) {
      const branches = hooks.getBranchesByProject().get(projectId) ?? [];
      for (const b of branches) {
        if (b.workspaceName === workspaceName) {
          peerBranchIds.push(b.id);
          this.handleWorkspaceStatusChange(projectId, b.id, 'starting');
        }
      }
    }

    try {
      await commands.resumeWorkspace(workspaceName);
    } catch (e) {
      console.error('[workspaceLifecycle] Failed to resume workspace:', e);
      const message = this.errorMessage(e);
      for (const id of peerBranchIds) {
        this.handleWorkspaceStatusChange(projectId, id, 'error');
        this.workspaceErrors = new Map(this.workspaceErrors).set(id, message);
      }
      this.version++;
      toast.error('Unable to resume workspace', {
        description: message,
        duration: Infinity,
      });
    }
  }

  /**
   * A transfer of `projectId` ended. Branches it left unprovisioned (a remote
   * workspace still to start, a local branch without a worktree) were skipped
   * while it ran, and errors from the old location no longer apply: clear
   * them, and hold the project until its refetch lands. What the store holds
   * until then may predate the switch (the commit's own refetch is
   * frame-buffered), so the kickoff provisions from the fresh lists instead.
   * Branch ids survive a transfer, so the ones held now are the ones to set
   * up with prerun actions.
   */
  transferFinished(projectId: string): void {
    const hooks = this.hooks;
    if (!hooks || hooks.isProjectTransferring(projectId)) return;
    for (const branch of hooks.getBranchesByProject().get(projectId) ?? []) {
      this.clearBranchState(branch.id);
      this.prerunOnSetup.add(branch.id);
    }
    this.awaitingTransferRefresh.add(projectId);
    void hooks
      .refreshTransferredProject(projectId)
      .catch((e) => {
        console.error('[workspaceLifecycle] Failed to refresh a transferred project:', e);
      })
      .finally(() => {
        if (this.hooks !== hooks) return;
        this.awaitingTransferRefresh.delete(projectId);
        this.scheduleKickoff();
      });
  }

  clearBranchState(branchId: string): void {
    this.pendingSetupBranches.delete(branchId);
    this.queuedSetupBranches.delete(branchId);
    this.prerunOnSetup.delete(branchId);

    this.localSetupTaskQueue = this.localSetupTaskQueue.filter(
      (task) => task.branchId !== branchId
    );

    let changed = false;
    if (this.worktreeErrors.has(branchId)) {
      const next = new Map(this.worktreeErrors);
      next.delete(branchId);
      this.worktreeErrors = next;
      changed = true;
    }
    if (this.workspaceErrors.has(branchId)) {
      const next = new Map(this.workspaceErrors);
      next.delete(branchId);
      this.workspaceErrors = next;
      changed = true;
    }
    if (changed) this.version++;
  }

  /** Whether setup must leave the project's branches alone for now: it is
   *  deleting, transferring, or waiting on its post-transfer refetch. */
  private isProjectHeld(projectId: string): boolean {
    const hooks = this.hooks;
    return (
      this.awaitingTransferRefresh.has(projectId) ||
      !!hooks?.isProjectDeleting(projectId) ||
      !!hooks?.isProjectTransferring(projectId)
    );
  }

  private setWorkspaceError(branchId: string, message: string): void {
    if (this.workspaceErrors.get(branchId) === message) return;
    this.workspaceErrors = new Map(this.workspaceErrors).set(branchId, message);
    this.version++;
  }

  private clearWorkspaceError(branchId: string): void {
    if (!this.workspaceErrors.has(branchId)) return;
    const next = new Map(this.workspaceErrors);
    next.delete(branchId);
    this.workspaceErrors = next;
    this.version++;
  }

  /** Every failure lands on the card. Only one the backend marks as the
   *  user's own ask is toasted: its automatic retries, startup pass and
   *  poll-triggered runs fail without interrupting anyone. */
  private applySetupErrorEvent(event: WorkspaceSetupErrorEvent): void {
    const { branchId, error } = event;
    this.setupErrorEventSeqByBranch.set(branchId, ++this.setupErrorEventSeq);
    if (error == null) {
      this.clearWorkspaceError(branchId);
      if (event.notice) {
        toast.warning(event.noticeTitle ?? 'Workspace branch was given existing commits', {
          description: event.notice,
          duration: Infinity,
        });
      }
      return;
    }
    this.setWorkspaceError(branchId, error);
    if (event.userRequested) {
      toast.error('Unable to start workspace', { description: error, duration: Infinity });
    }
  }

  private async seedSetupErrors(generation: number): Promise<void> {
    const seqAtRequest = this.setupErrorEventSeq;
    let errors: Record<string, string>;
    try {
      errors = await commands.getWorkspaceSetupErrors();
    } catch (e) {
      console.error('[workspaceLifecycle] Failed to load workspace setup errors:', e);
      return;
    }
    if (this.generation !== generation) return;
    for (const [branchId, message] of Object.entries(errors ?? {})) {
      // An event since the request is newer than the snapshot.
      if ((this.setupErrorEventSeqByBranch.get(branchId) ?? 0) > seqAtRequest) continue;
      this.setWorkspaceError(branchId, message);
    }
  }

  private errorMessage(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  private toWorkspaceStatus(status: string): WorkspaceStatus | null {
    return status === 'starting' ||
      status === 'running' ||
      status === 'stopped' ||
      status === 'suspended' ||
      status === 'error'
      ? status
      : null;
  }

  private collectPollableRemoteBranches(): Array<{
    projectId: string;
    branchId: string;
    status: WorkspaceStatus;
  }> {
    const hooks = this.hooks;
    if (!hooks) return [];

    const targets: Array<{ projectId: string; branchId: string; status: WorkspaceStatus }> = [];

    for (const [projectId, branches] of hooks.getBranchesByProject().entries()) {
      // A transfer creates and deletes its workspaces itself.
      if (hooks.isProjectTransferring(projectId)) continue;
      for (const branch of branches) {
        if (
          branch.branchType === 'remote' &&
          (branch.workspaceStatus === 'starting' || branch.workspaceStatus === 'running')
        ) {
          targets.push({ projectId, branchId: branch.id, status: branch.workspaceStatus });
        }
      }
    }

    return targets;
  }

  private async pollWorkspaceStatuses(): Promise<void> {
    if (this.pollInFlight) return;

    const targets = this.collectPollableRemoteBranches();
    if (targets.length === 0) {
      this.stopPolling();
      return;
    }

    const activeBranchIds = targets.map((target) => target.branchId);

    this.pollInFlight = true;
    try {
      const resultMap = await commands.pollAllWorkspaceStatuses(activeBranchIds);

      for (const target of targets) {
        const result = resultMap[target.branchId];
        if (!result) continue;

        const nextStatus = this.toWorkspaceStatus(result.status);
        if (!nextStatus) continue;

        this.handleWorkspaceStatusChange(
          target.projectId,
          target.branchId,
          nextStatus,
          result.workstationId
        );
      }
    } catch (e) {
      console.error('[workspaceLifecycle] batch workspace status poll failed:', e);
    } finally {
      this.pollInFlight = false;
    }

    this.scheduleNextPoll();
  }

  private scheduleNextPoll(): void {
    if (this.pollTimer) {
      clearTimeout(this.pollTimer);
      this.pollTimer = null;
    }

    const targets = this.collectPollableRemoteBranches();
    if (targets.length === 0) return;

    const hasStarting = targets.some((t) => t.status === 'starting');
    const intervalMs = hasStarting ? this.STARTING_POLL_MS : this.RUNNING_POLL_MS;

    this.pollTimer = setTimeout(() => {
      this.pollTimer = null;
      void this.pollWorkspaceStatuses();
    }, intervalMs);
  }

  private stopPolling(): void {
    if (this.pollTimer) {
      clearTimeout(this.pollTimer);
      this.pollTimer = null;
    }
    this.pollInFlight = false;
  }

  private ensurePolling(): void {
    if (!this.pollTimer && !this.pollInFlight) {
      void this.pollWorkspaceStatuses();
    }
  }

  private kickOffPendingBranchSetup(): void {
    const hooks = this.hooks;
    if (!hooks) return;

    for (const [projectId, branches] of hooks.getBranchesByProject().entries()) {
      if (this.isProjectHeld(projectId)) continue;
      for (const branch of branches) {
        this.enqueueBranchSetup(projectId, branch);
      }
    }
  }

  private enqueueBranchSetup(projectId: string, branch: Branch): void {
    const branchId = branch.id;
    if (this.pendingSetupBranches.has(branchId) || this.queuedSetupBranches.has(branchId)) return;

    // A project's branches all live where it does. One that doesn't is from
    // a list read before a transfer switched the project over, and setting
    // it up would provision the old location again.
    const location = this.hooks?.getProjectLocation(projectId) ?? null;
    if (location && branch.branchType !== location) return;
    const runPrerun = this.prerunOnSetup.delete(branchId);

    if (branch.branchType === 'local') {
      if (branch.worktreePath || this.worktreeErrors.has(branchId)) return;
      this.queuedSetupBranches.add(branchId);
      this.localSetupTaskQueue.push({
        branchId,
        run: async () => {
          await this.setupBranchWorktree(branchId, projectId, { runPrerun });
        },
      });
      this.pumpLocalSetupQueue();
      return;
    }

    if (branch.branchType === 'remote' && branch.workspaceStatus === 'starting') {
      this.requestRemoteSetup(branchId);
    }
  }

  /**
   * Ask the backend to set a remote branch up. Only asks: the call resolves
   * as soon as the backend's run is under way (or joined), the status poll
   * reports how it ends, and a failure's reason arrives as a
   * `workspace-setup-error` event. A rejection means the request itself
   * could not be made, and says nothing about the branch. Asking again for a
   * branch that is still `starting` is harmless: it joins the same run.
   */
  private requestRemoteSetup(branchId: string): void {
    this.pendingSetupBranches.add(branchId);
    // A branch can turn `starting` in a refetched list with the poller
    // stopped; its outcome comes from polls.
    this.ensurePolling();
    commands
      .startWorkspace(branchId)
      .catch((e) => {
        console.error('[workspaceLifecycle] Failed to request workspace setup:', e);
        // A reason a Retry cleared here may still stand in the backend.
        void this.seedSetupErrors(this.generation);
      })
      .finally(() => {
        this.pendingSetupBranches.delete(branchId);
      });
  }

  private pumpLocalSetupQueue(): void {
    while (
      this.activeLocalSetupCount < this.MAX_LOCAL_SETUP_CONCURRENCY &&
      this.localSetupTaskQueue.length > 0
    ) {
      const task = this.localSetupTaskQueue.shift();
      if (!task) break;

      this.activeLocalSetupCount += 1;
      task
        .run()
        .catch((e) => {
          console.error('[workspaceLifecycle] Local branch setup task failed:', e);
        })
        .finally(() => {
          this.activeLocalSetupCount = Math.max(0, this.activeLocalSetupCount - 1);
          this.queuedSetupBranches.delete(task.branchId);
          this.pumpLocalSetupQueue();
        });
    }
  }

  private async setupBranchWorktree(
    branchId: string,
    projectId: string,
    opts?: { runPrerun?: boolean }
  ): Promise<void> {
    if (this.pendingSetupBranches.has(branchId)) return;
    this.pendingSetupBranches.add(branchId);

    if (this.worktreeErrors.has(branchId)) {
      const nextErrors = new Map(this.worktreeErrors);
      nextErrors.delete(branchId);
      this.worktreeErrors = nextErrors;
      this.version++;
    }

    try {
      const updated = opts?.runPrerun
        ? await commands.setupWorktreeAndRunPrerun(
            branchId,
            getPreferredAgent(agentState.providers) ?? undefined
          )
        : await commands.setupWorktree(branchId);
      const hooks = this.hooks;
      if (hooks) {
        const current = hooks.getBranchesByProject();
        const branches = current.get(projectId) || [];
        hooks.setBranchesByProject(
          new Map(current).set(
            projectId,
            branches.map((branch) => (branch.id === updated.id ? updated : branch))
          )
        );
      }

      // NOTE: prerun actions are only triggered here when opts.runPrerun
      // is set (the retry path, and a branch a transfer handed over without
      // a worktree). The normal creation paths run them in the backend
      // (create_project / add_project_repo / MCP), as transfer_project does
      // for the branches it carried. Queued sessions are likewise started by
      // the backend when it links the worktree, not from here.
    } catch (e) {
      console.error('[workspaceLifecycle] Failed to setup worktree:', e);
      const errMsg = e instanceof Error ? e.message : typeof e === 'string' ? e : String(e);
      this.worktreeErrors = new Map(this.worktreeErrors).set(branchId, errMsg);
      this.version++;
      throw e;
    } finally {
      this.pendingSetupBranches.delete(branchId);
    }
  }
}

export const workspaceLifecycle = new WorkspaceLifecycleController();
