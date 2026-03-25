import type { Branch, WorkspaceStatus } from '../../types';
import * as commands from '../../api/commands';
import { runPrerunActions } from '../actions/actions';
import { alerts } from '../../shared/alerts.svelte';

type BranchMap = Map<string, Branch[]>;

interface LocalSetupTask {
  branchId: string;
  run: () => Promise<void>;
}

interface RemoteSetupTask {
  branchId: string;
  workspaceName: string | null;
  run: () => Promise<void>;
}

interface WorkspaceLifecycleHooks {
  getBranchesByProject: () => BranchMap;
  setBranchesByProject: (next: BranchMap) => void;
  isProjectDeleting: (projectId: string) => boolean;
}

class WorkspaceLifecycleController {
  private hooks: WorkspaceLifecycleHooks | null = null;

  private worktreeErrors = $state<Map<string, string>>(new Map());
  private workspaceErrors = $state<Map<string, string>>(new Map());
  private version = $state(0);

  private pendingSetupBranches = new Set<string>();
  private queuedSetupBranches = new Set<string>();

  private activeLocalSetupCount = 0;
  private activeRemoteSetupCount = 0;
  private readonly MAX_LOCAL_SETUP_CONCURRENCY = 1;
  private readonly MAX_REMOTE_SETUP_CONCURRENCY = 4;
  private activeRemoteWorkspaceStarts = new Set<string>();

  private localSetupTaskQueue: LocalSetupTask[] = [];
  private remoteSetupTaskQueue: RemoteSetupTask[] = [];

  // Single adaptive poller: 3s while any workspace is starting, 30s while
  // any is running, stopped otherwise.
  private readonly STARTING_POLL_MS = 3000;
  private readonly RUNNING_POLL_MS = 30_000;
  private readonly STARTING_TIMEOUT_MS = 5 * 60 * 1000;

  private pollTimer: ReturnType<typeof setTimeout> | null = null;
  private pollInFlight = false;
  private startingTimestamps = new Map<string, number>();
  private kickoffTimer: ReturnType<typeof setTimeout> | null = null;

  start(hooks: WorkspaceLifecycleHooks): void {
    this.hooks = hooks;
    this.ensurePolling();
  }

  stop(): void {
    this.stopPolling();
    if (this.kickoffTimer) {
      clearTimeout(this.kickoffTimer);
      this.kickoffTimer = null;
    }
    this.pendingSetupBranches.clear();
    this.queuedSetupBranches.clear();
    this.activeRemoteWorkspaceStarts.clear();
    this.startingTimestamps.clear();
    this.localSetupTaskQueue = [];
    this.remoteSetupTaskQueue = [];
    this.activeLocalSetupCount = 0;
    this.activeRemoteSetupCount = 0;
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
    const nextBranches = branches.map((branch) => {
      if (branch.id !== branchId) return branch;
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

    if (workspaceStatus !== 'error' && this.workspaceErrors.has(branchId)) {
      const nextWorkspaceErrors = new Map(this.workspaceErrors);
      nextWorkspaceErrors.delete(branchId);
      this.workspaceErrors = nextWorkspaceErrors;
      this.version++;
    }

    // Track starting timestamps for timeout logic.
    if (workspaceStatus === 'starting') {
      if (!this.startingTimestamps.has(branchId)) {
        this.startingTimestamps.set(branchId, Date.now());
      }
    } else {
      this.startingTimestamps.delete(branchId);
    }

    // Kick the poller when a workspace enters an active state.
    if (workspaceStatus === 'starting' || workspaceStatus === 'running') {
      this.ensurePolling();
    }
  }

  async retryWorktree(branchId: string, projectId: string): Promise<void> {
    await this.setupBranchWorktree(branchId, projectId);
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
        this.workspaceErrors = new Map(this.workspaceErrors).set(id, message);
        this.handleWorkspaceStatusChange(projectId, id, 'error');
      }
      this.version++;
      alerts.show({
        tone: 'error',
        title: 'Unable to resume workspace',
        message,
        durationMs: 0,
      });
    }
  }

  clearBranchState(branchId: string): void {
    this.pendingSetupBranches.delete(branchId);
    this.queuedSetupBranches.delete(branchId);
    this.startingTimestamps.delete(branchId);

    this.localSetupTaskQueue = this.localSetupTaskQueue.filter(
      (task) => task.branchId !== branchId
    );
    this.remoteSetupTaskQueue = this.remoteSetupTaskQueue.filter(
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

    // Apply 5-minute timeout to starting workspaces before polling.
    const now = Date.now();
    const activeBranchIds: string[] = [];
    for (const target of targets) {
      if (target.status === 'starting') {
        const startedAt = this.startingTimestamps.get(target.branchId);
        if (startedAt && now - startedAt > this.STARTING_TIMEOUT_MS) {
          this.startingTimestamps.delete(target.branchId);
          this.handleWorkspaceStatusChange(target.projectId, target.branchId, 'error');
          continue;
        }
      }
      activeBranchIds.push(target.branchId);
    }

    if (activeBranchIds.length === 0) {
      this.scheduleNextPoll();
      return;
    }

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
      if (hooks.isProjectDeleting(projectId)) continue;
      for (const branch of branches) {
        this.enqueueBranchSetup(projectId, branch);
      }
    }
  }

  private enqueueBranchSetup(projectId: string, branch: Branch): void {
    const branchId = branch.id;
    if (this.pendingSetupBranches.has(branchId) || this.queuedSetupBranches.has(branchId)) return;

    if (branch.branchType === 'local') {
      if (branch.worktreePath || this.worktreeErrors.has(branchId)) return;
      this.queuedSetupBranches.add(branchId);
      this.localSetupTaskQueue.push({
        branchId,
        run: async () => {
          await this.setupBranchWorktree(branchId, projectId);
        },
      });
      this.pumpLocalSetupQueue();
      return;
    }

    if (branch.branchType === 'remote' && branch.workspaceStatus === 'starting') {
      this.queuedSetupBranches.add(branchId);
      this.remoteSetupTaskQueue.push({
        branchId,
        workspaceName: branch.workspaceName,
        run: async () => {
          this.pendingSetupBranches.add(branchId);
          if (this.workspaceErrors.has(branchId)) {
            const nextWorkspaceErrors = new Map(this.workspaceErrors);
            nextWorkspaceErrors.delete(branchId);
            this.workspaceErrors = nextWorkspaceErrors;
            this.version++;
          }

          try {
            await commands.startWorkspace(branchId);
          } catch (e) {
            console.error('[workspaceLifecycle] Failed to start workspace:', e);
            const message = this.errorMessage(e);
            this.workspaceErrors = new Map(this.workspaceErrors).set(branchId, message);
            this.version++;
            this.handleWorkspaceStatusChange(projectId, branchId, 'error');
            alerts.show({
              tone: 'error',
              title: 'Unable to start workspace',
              message,
              durationMs: 0,
            });
          } finally {
            this.pendingSetupBranches.delete(branchId);
          }
        },
      });
      this.pumpRemoteSetupQueue();
    }
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

  private pumpRemoteSetupQueue(): void {
    while (
      this.activeRemoteSetupCount < this.MAX_REMOTE_SETUP_CONCURRENCY &&
      this.remoteSetupTaskQueue.length > 0
    ) {
      const taskIndex = this.remoteSetupTaskQueue.findIndex(
        (task) => !task.workspaceName || !this.activeRemoteWorkspaceStarts.has(task.workspaceName)
      );
      if (taskIndex === -1) break;
      const task = this.remoteSetupTaskQueue.splice(taskIndex, 1)[0];

      this.activeRemoteSetupCount += 1;
      if (task.workspaceName) {
        this.activeRemoteWorkspaceStarts.add(task.workspaceName);
      }
      task
        .run()
        .catch((e) => {
          console.error('[workspaceLifecycle] Remote workspace setup task failed:', e);
        })
        .finally(() => {
          this.activeRemoteSetupCount = Math.max(0, this.activeRemoteSetupCount - 1);
          if (task.workspaceName) {
            this.activeRemoteWorkspaceStarts.delete(task.workspaceName);
          }
          this.queuedSetupBranches.delete(task.branchId);
          this.pumpRemoteSetupQueue();
        });
    }
  }

  private async setupBranchWorktree(branchId: string, projectId: string): Promise<void> {
    if (this.pendingSetupBranches.has(branchId)) return;
    this.pendingSetupBranches.add(branchId);

    if (this.worktreeErrors.has(branchId)) {
      const nextErrors = new Map(this.worktreeErrors);
      nextErrors.delete(branchId);
      this.worktreeErrors = nextErrors;
      this.version++;
    }

    try {
      const updated = await commands.setupWorktree(branchId);
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

      setTimeout(() => {
        runPrerunActions(branchId).catch((e) => {
          console.error('[workspaceLifecycle] Failed to run prerun actions:', e);
        });
      }, 150);
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
