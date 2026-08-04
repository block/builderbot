<!--
  ProjectHome.svelte - Project workspace page

  In app navigation this is the "project page". It can render a single selected
  project (detail view) or multiple projects when no filter is provided.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import CirclePause from '@lucide/svelte/icons/circle-pause';
  import Cloud from '@lucide/svelte/icons/cloud';
  import Pause from '@lucide/svelte/icons/pause';
  import Plus from '@lucide/svelte/icons/plus';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import { getWindowSync } from '../../transport';
  import type {
    Project,
    ProjectRepo,
    Branch,
    WorkspaceStatus,
    StoreIncompatibility,
  } from '../../types';
  import * as commands from '../../api/commands';
  import { listenToRepoActionsDetection } from '../actions/actions';
  import { projectDisplayName } from '../../shared/utils';
  import { goHome, selectProject } from '../layout/navigation.svelte';
  import TopBarPortal from '../layout/TopBarPortal.svelte';
  import ProjectSection from './ProjectSection.svelte';
  import type { RepoSelection as RepoPickerSelection } from '../../shared/githubUrl';
  import AddRepoModal from './AddRepoModal.svelte';
  import NewProjectModal from './NewProjectModal.svelte';
  import SplashScreen from './SplashScreen.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { Button } from '$lib/components/ui/button';
  import { toast } from 'svelte-sonner';
  import { workspaceLifecycle } from './workspaceLifecycle.svelte';
  import { projectActions } from './projectActions.svelte';
  import { projectRunActionsStore } from '../../stores/projectRunActions.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import {
    canDeleteProjectWithoutConfirmation,
    computeSafeToDeleteSignature,
  } from './projectDeleteSafety';

  interface Props {
    selectedProjectId?: string | null;
  }

  let { selectedProjectId = null }: Props = $props();

  // Data comes from the shared projectsData store (module-scoped, survives
  // route changes); this view only owns UI state and view-lifecycle policy.
  let projects = $derived(projectsDataStore.projects);
  let branchesByProject = $derived(projectsDataStore.branchesByProject);
  let reposByProject = $derived(projectsDataStore.reposByProject);
  let repoCountsByProject = $derived(projectsDataStore.repoCountsByProject);
  let deletingProjectNames = $derived(projectsDataStore.deletingProjectNames);

  /** Repos keyed by repo id for ProjectSection's branch.projectRepoId lookups. */
  let reposById = $derived.by(() => {
    const map = new Map<string, ProjectRepo>();
    for (const repos of reposByProject.values()) {
      for (const repo of repos) map.set(repo.id, repo);
    }
    return map;
  });

  // The store-status check runs before the first ensureLoaded call; hold the
  // loading state until it settles so the splash screen doesn't flash.
  let storeCheckPending = $state(true);
  let loading = $derived(storeCheckPending || projectsDataStore.loading);
  // Store-status/reset failures are view-local; load failures come from the store.
  let viewError = $state<string | null>(null);
  let error = $derived(viewError ?? projectsDataStore.error);

  let initialLoadComplete = false;
  let lastSelectedProjectId: string | null = null;

  // Startup queued-session drain: once per branch per mount, batched through
  // the idle queue. Branches that become ready later (worktree setup or
  // workspace start completing) are drained by workspaceLifecycle itself.
  let queuedSessionDrainCancel: (() => void) | null = null;
  const drainedSessionBranchIds = new Set<string>();
  const pendingDrainBranchIds = new Set<string>();

  // Store health — if non-null the DB needs a reset before we can proceed
  let storeIncompat = $state<StoreIncompatibility | null>(null);
  let resetting = $state(false);

  // Modal state
  let showNewProjectModal = $state(false);
  let showAddRepoModal = $state(false);

  // Project-detail top-bar title handoff.
  let mainPanelEl = $state<HTMLDivElement | null>(null);
  let projectTitleElement = $state<HTMLHeadingElement | null>(null);
  let showTopBarProjectName = $state(false);

  // Delete confirmation state (remove-project confirmation lives in the
  // shared projectActions module + App-level ProjectDeleteDialog)
  let branchToDelete = $state<{ branch: Branch; project: Project } | null>(null);
  let deletingBranches = $state<Set<string>>(new Set());
  // Guards the delete shortcut while the async safe-to-delete check is in flight,
  // before projectActions.pendingDelete/deletingProjectNames are set, so a held
  // key only deletes once.
  let deleteShortcutPending = $state(false);

  // Setup errors come from the shared workspace lifecycle orchestrator.
  let worktreeErrors = $derived(workspaceLifecycle.getWorktreeErrors());
  let workspaceErrors = $derived(workspaceLifecycle.getWorkspaceErrors());

  // Action detection state
  let detectingProjectIds = $state<Set<string>>(new Set());

  onMount(() => {
    // Backend/window listeners for the shared data (pr-status-changed,
    // session-status-changed, project-setup-progress, cache-stale) live in
    // the projectsData store, started once from App.svelte.
    workspaceLifecycle.start({
      getBranchesByProject: () => projectsDataStore.branchesByProject,
      setBranchesByProject: (next) => projectsDataStore.setBranchesByProject(next),
      isProjectDeleting: (projectId) => projectsDataStore.isProjectDeleting(projectId),
    });
    checkStoreAndLoad();
    void projectRunActionsStore.startListening();

    const onNewProject = () => handleNewProject();
    window.addEventListener('staged:new-project', onNewProject);
    const onDeleteCurrentProject = (event: Event) => handleDeleteCurrentProjectShortcut(event);
    window.addEventListener('staged:delete-current-project', onDeleteCurrentProject);

    const unlistenDetection = listenToRepoActionsDetection((event) => {
      const matchingProjectIds = projectsDataStore.projects
        .filter((p) => p.githubRepo === event.githubRepo && p.subpath === event.subpath)
        .map((p) => p.id);
      if (matchingProjectIds.length === 0) return;

      const next = new Set(detectingProjectIds);
      for (const projectId of matchingProjectIds) {
        if (event.detecting) {
          next.add(projectId);
        } else {
          next.delete(projectId);
        }
      }
      detectingProjectIds = next;
    });

    return () => {
      window.removeEventListener('staged:new-project', onNewProject);
      window.removeEventListener('staged:delete-current-project', onDeleteCurrentProject);
      unlistenDetection();
      cancelQueuedSessionDrain();
      workspaceLifecycle.stop();
      projectRunActionsStore.stopListening();
    };
  });

  async function checkStoreAndLoad() {
    try {
      const status = await commands.getStoreStatus();
      if (status) {
        storeIncompat = status;
        return;
      }
      // On a revisit the store resolves instantly from memory and revalidates
      // in the background; foreground-refresh the selected project only when
      // this call didn't just perform the full eager load.
      const wasLoaded = projectsDataStore.loaded;
      const load = projectsDataStore.ensureLoaded();
      storeCheckPending = false;
      await load;
      lastSelectedProjectId = selectedProjectId;
      initialLoadComplete = true;
      if (selectedProjectId && wasLoaded) {
        void projectsDataStore.hydrateProject(selectedProjectId);
      }
      void hydrateActionDetection();
    } catch (e) {
      viewError = e instanceof Error ? e.message : String(e);
    } finally {
      storeCheckPending = false;
    }
  }

  async function handleResetStore() {
    resetting = true;
    try {
      await commands.confirmResetStore();
      storeIncompat = null;
      viewError = null;
      await projectsDataStore.refresh();
    } catch (e) {
      viewError = e instanceof Error ? e.message : String(e);
    } finally {
      resetting = false;
    }
  }

  function handleClose() {
    getWindowSync().close();
  }

  function scheduleDeferredTask(callback: () => void, timeout = 1500): () => void {
    const schedule =
      typeof requestIdleCallback === 'function'
        ? (cb: () => void) => requestIdleCallback(cb, { timeout })
        : (cb: () => void) => setTimeout(cb, 0) as unknown as number;
    const cancel =
      typeof cancelIdleCallback === 'function'
        ? (handle: number) => cancelIdleCallback(handle)
        : (handle: number) => clearTimeout(handle);

    const handle = schedule(callback);
    return () => cancel(handle);
  }

  function cancelQueuedSessionDrain() {
    queuedSessionDrainCancel?.();
    queuedSessionDrainCancel = null;
    pendingDrainBranchIds.clear();
  }

  function scheduleQueuedSessionDrain(branchMap: Map<string, Branch[]>) {
    for (const branches of branchMap.values()) {
      for (const branch of branches) {
        const isLocalReady = branch.branchType === 'local' && branch.worktreePath;
        const isRemoteReady =
          branch.branchType === 'remote' && branch.workspaceStatus === 'running';
        if ((isLocalReady || isRemoteReady) && !drainedSessionBranchIds.has(branch.id)) {
          drainedSessionBranchIds.add(branch.id);
          pendingDrainBranchIds.add(branch.id);
        }
      }
    }

    if (pendingDrainBranchIds.size === 0 || queuedSessionDrainCancel) return;

    queuedSessionDrainCancel = scheduleDeferredTask(() => {
      queuedSessionDrainCancel = null;
      const branchIds = Array.from(pendingDrainBranchIds);
      pendingDrainBranchIds.clear();
      for (const branchId of branchIds) {
        commands.drainQueuedSessions(branchId).catch((e) => {
          console.error('[ProjectHome] Failed to drain queued sessions on startup:', e);
        });
      }
    }, 3000);
  }

  // View-lifecycle side effects wired off the store's branch data — worktree/
  // workspace setup, the startup queued-session drain, and run-action
  // hydration. All three dedupe internally, so re-running on every branch map
  // reassignment is cheap. They stay out of the store deliberately: they are
  // this view's policies, not properties of the data.
  $effect(() => {
    const branchMap = projectsDataStore.branchesByProject;
    for (const [projectId, branches] of branchMap) {
      if (branches.length > 0) {
        workspaceLifecycle.enqueueInitialSetup(projectId, branches);
      }
    }
    scheduleQueuedSessionDrain(branchMap);
    projectRunActionsStore.hydrateFromProjectBranches(branchMap).catch(console.error);
  });

  let actionDetectionToken = 0;

  async function hydrateActionDetection() {
    const token = ++actionDetectionToken;
    try {
      const contexts = await commands.listActionContexts();
      if (token !== actionDetectionToken) return;
      detectingProjectIds = new Set(
        projectsDataStore.projects
          .filter((project) =>
            contexts.some(
              (context) =>
                context.detectingActions &&
                context.githubRepo === project.githubRepo &&
                context.subpath === project.subpath
            )
          )
          .map((project) => project.id)
      );
    } catch (e) {
      console.error('[ProjectHome] Failed to load action contexts:', e);
    }
  }

  $effect(() => {
    const projectId = selectedProjectId;
    if (!initialLoadComplete) return;
    if (projectId === lastSelectedProjectId) return;
    lastSelectedProjectId = projectId;
    // Foreground-refresh the newly selected project; ensureLoaded's
    // background revalidation drips the rest through the idle queue.
    void projectsDataStore.ensureLoaded();
    if (projectId) {
      void projectsDataStore.hydrateProject(projectId);
    }
    void hydrateActionDetection();
  });

  let visibleProjects = $derived(
    selectedProjectId ? projects.filter((project) => project.id === selectedProjectId) : projects
  );
  let hasContent = $derived(visibleProjects.length > 0);
  let selectedProject = $derived(
    selectedProjectId ? projects.find((project) => project.id === selectedProjectId) || null : null
  );

  function updateTopBarProjectNameVisibility(): void {
    if (!selectedProject || !mainPanelEl || !projectTitleElement) {
      showTopBarProjectName = false;
      return;
    }

    const panelTop = mainPanelEl.getBoundingClientRect().top;
    const titleBottom = projectTitleElement.getBoundingClientRect().bottom;
    showTopBarProjectName = titleBottom <= panelTop + 1;
  }

  function setProjectTitleElement(element: HTMLHeadingElement | null): void {
    projectTitleElement = element;
    updateTopBarProjectNameVisibility();
  }

  $effect(() => {
    selectedProject;
    projectTitleElement;
    mainPanelEl;
    loading;

    if (typeof requestAnimationFrame !== 'function') {
      updateTopBarProjectNameVisibility();
      return;
    }

    const frame = requestAnimationFrame(updateTopBarProjectNameVisibility);
    return () => cancelAnimationFrame(frame);
  });

  let selectedProjectDeleting = $derived(
    selectedProject ? deletingProjectNames.has(selectedProject.id) : false
  );
  let selectedProjectDetecting = $derived(
    selectedProject ? detectingProjectIds.has(selectedProject.id) : false
  );
  // Track which projects are safe to delete (for button styling)
  let safeToDeleteProjects = $state<Set<string>>(new Set());
  let selectedProjectSafeToDelete = $derived(
    selectedProject ? safeToDeleteProjects.has(selectedProject.id) : false
  );
  let selectedProjectCanAddRepo = $derived(selectedProject ? canAddRepo(selectedProject) : false);
  let selectedProjectAddRepoHint = $derived(
    selectedProject && selectedProject.location === 'remote' ? addRepoHint(selectedProject) : null
  );
  let selectedProjectAddRepoDisabled = $derived(
    !selectedProject || selectedProjectDeleting || !selectedProjectCanAddRepo
  );
  let selectedProjectExcludeRepos = $derived(
    selectedProject
      ? new Set(
          (reposByProject.get(selectedProject.id) ?? []).map(
            (repo) => `${repo.githubRepo}\x00${repo.subpath ?? ''}`
          )
        )
      : new Set<string>()
  );

  // Update safe-to-delete status when branches change.
  // Only check visible projects — calling hasUnpushedCommits for every
  // project wastes IPC round-trips (especially expensive for remote branches)
  // and the result is only consumed in the visibleProjects render loop.
  // Signature of the last inputs the safe-to-delete check actually ran against.
  // Background hydration/pollers reassign visibleProjects/branchesByProject/
  // repoCountsByProject many times per switch even when the fields this check
  // depends on are unchanged; deduping on the signature keeps the expensive
  // per-branch git work from re-firing on every reassignment.
  let lastSafeSignature: string | null = null;
  // Set when a branch's `prState` actually flips (e.g. OPEN → MERGED) between
  // runs — with the pr-status-changed listener living in the projectsData
  // store, the transition is detected here by diffing against the previous
  // run. A live transition while parked on a project is not a switch-time
  // hydration storm, so the recompute below takes a prompt (next-tick) path
  // instead of the idle window — keeping the delete button in step with the
  // branch card's badge. Consumed (reset) by the effect once it acts on it.
  let prStateTransitionPending = false;
  let lastKnownPrStates = new Map<string, Branch['prState']>();
  $effect(() => {
    // Read reactive deps synchronously so the effect re-subscribes correctly.
    const projectsSnapshot = visibleProjects;
    const branches = branchesByProject;
    const repoCounts = repoCountsByProject;

    // Diff prState across all branches (not just visible ones) so a flip
    // elsewhere still arms the fast path for the next genuine recompute.
    const nextPrStates = new Map<string, Branch['prState']>();
    for (const branchList of branches.values()) {
      for (const b of branchList) {
        nextPrStates.set(b.id, b.prState);
        const previous = lastKnownPrStates.get(b.id);
        if (previous !== undefined && previous !== b.prState) {
          prStateTransitionPending = true;
        }
      }
    }
    lastKnownPrStates = nextPrStates;

    const signature = computeSafeToDeleteSignature(projectsSnapshot, branches, repoCounts);
    if (signature === lastSafeSignature) {
      // Inputs relevant to the result are unchanged — skip the git work.
      // Leave any pending fast-path signal intact so the next genuine recompute
      // still picks it up.
      return;
    }

    // Consume the fast-path signal here (after the dedup gate) so a no-op
    // re-fire never burns it. A live PR-state transition recomputes promptly;
    // every other input change stays on the idle path that protects switches.
    const fastPath = prStateTransitionPending;
    prStateTransitionPending = false;

    // Bail out of stale work: if the effect re-fires (or the component tears
    // down) before this run resolves, `stale` flips so we neither spawn the
    // git loop's tail nor clobber newer state.
    let stale = false;
    let idleHandle: number | undefined;

    const updateSafeStatus = async () => {
      // Parallelize across projects so the project-home grid doesn't serialize
      // every project's git work.
      const results = await Promise.all(
        projectsSnapshot.map(async (project) => {
          const projectBranches = branches.get(project.id) || [];
          const repoCount = repoCounts.get(project.id) || 0;

          // Don't show red styling for projects with no repos — there's nothing
          // to call attention to when no repositories have been added yet.
          if (repoCount === 0) {
            return { id: project.id, safe: false };
          }

          const safe = await canDeleteProjectWithoutConfirmation({
            branches: projectBranches,
            repoCount,
            hasUnpushedCommits: commands.hasUnpushedCommits,
          });
          return { id: project.id, safe };
        })
      );

      if (stale) return;

      const nextSafe = new Set<string>();
      for (const { id, safe } of results) {
        if (safe) nextSafe.add(id);
      }
      safeToDeleteProjects = nextSafe;
      // Only record the signature once the check actually completes. If this
      // run is cancelled (re-fire/teardown) before it gets here, the signature
      // stays unchanged so the next fire reschedules instead of dropping the
      // check permanently for that signature.
      lastSafeSignature = signature;
    };

    // A live PR-state transition (fast path) settles on the next tick so the
    // delete button keeps pace with the branch card's badge. Everything else
    // defers off the critical render path: let the switch's keyed-block swap
    // flush first, then settle the cosmetic styling during idle. The timeout
    // guarantees the check still runs even while the main thread stays busy
    // through the post-switch hydration window (otherwise an idle callback can
    // be deferred indefinitely under sustained load).
    const useIdle = !fastPath && typeof requestIdleCallback === 'function';
    const schedule = useIdle
      ? (cb: () => void) => requestIdleCallback(cb, { timeout: 2000 })
      : (cb: () => void) => setTimeout(cb, 0) as unknown as number;
    const cancel = useIdle
      ? (handle: number) => cancelIdleCallback(handle)
      : (handle: number) => clearTimeout(handle);

    idleHandle = schedule(() => {
      void updateSafeStatus();
    });

    return () => {
      stale = true;
      if (idleHandle !== undefined) cancel(idleHandle);
    };
  });

  $effect(() => {
    if (!loading && selectedProjectId && projects.length > 0 && !selectedProject) {
      goHome();
    }
  });

  // ── Project actions ──

  function handleNewProject() {
    showNewProjectModal = true;
  }

  function handleProjectCreated(project: Project) {
    // The store registers the project synchronously and hydrates branches and
    // repos in the background, so the modal closes instantly.
    projectsDataStore.projectCreated(project);
    showNewProjectModal = false;
    selectProject(project.id);
  }

  function handleDeleteCurrentProjectShortcut(event: Event) {
    if (
      !selectedProject ||
      deleteShortcutPending ||
      selectedProjectDeleting ||
      projectActions.pendingDelete ||
      branchToDelete ||
      showNewProjectModal ||
      showAddRepoModal
    ) {
      return;
    }

    event.preventDefault();
    deleteShortcutPending = true;
    void projectActions.requestRemoveProject(selectedProject).finally(() => {
      deleteShortcutPending = false;
    });
  }

  // ── Branch actions ──

  async function handleRepoSelected(projectId: string, selection: RepoPickerSelection) {
    try {
      await commands.addProjectRepo(
        projectId,
        selection.nameWithOwner,
        selection.branchName,
        selection.subpath,
        undefined,
        selection.prNumber,
        selection.defaultBranch ?? undefined,
        selection.headRepo ?? undefined
      );
      if (selection.prNumber != null && selection.prTitle) {
        const noteTitle = `PR #${selection.prNumber}: ${selection.prTitle}`;
        await commands.createProjectNote(projectId, noteTitle, selection.prBody ?? '');
      }
      await projectsDataStore.refreshProject(projectId);
    } catch (e) {
      console.error('Failed to add repo:', e);
      const message = e instanceof Error ? e.message : String(e);
      toast.error('Unable to add repository', {
        description: message,
        duration: Infinity,
      });
    }
  }

  function canAddRepo(project: Project): boolean {
    if (project.location !== 'remote') return true;
    const branches = branchesByProject.get(project.id) || [];
    return branches.some((b) => b.branchType === 'remote' && b.workspaceStatus === 'running');
  }

  function addRepoHint(project: Project): string {
    if (project.location !== 'remote') return '';
    const branches = branchesByProject.get(project.id) || [];
    if (branches.length === 0) {
      return 'Create the initial remote branch and workspace before adding another repo.';
    }
    if (branches.some((b) => b.branchType === 'remote' && b.workspaceStatus === 'starting')) {
      return 'Workspace is provisioning. Wait until it is running, then add another repo.';
    }
    return 'Workspace must be running before adding another repo.';
  }

  function getRemoteWorkspaceStatus(project: Project | null): WorkspaceStatus | null {
    if (!project || project.location !== 'remote') return null;
    return (
      (branchesByProject.get(project.id) || []).find((b) => b.workspaceStatus)?.workspaceStatus ??
      null
    );
  }

  function getRemoteWorkstationName(project: Project | null): string | null {
    if (!project || project.location !== 'remote') return null;
    return (
      (branchesByProject.get(project.id) || []).find((b) => b.workspaceName)?.workspaceName ?? null
    );
  }

  function statusLabel(status: WorkspaceStatus | null): string {
    switch (status) {
      case 'starting':
        return 'Provisioning';
      case 'running':
        return 'Running';
      case 'stopped':
        return 'Stopped';
      case 'suspended':
        return 'Suspended';
      case 'error':
        return 'Error';
      default:
        return '';
    }
  }

  async function handleTopBarRepoSelected(selection: RepoPickerSelection) {
    const project = selectedProject;
    if (!project) return;
    showAddRepoModal = false;
    await handleRepoSelected(project.id, selection);
  }

  $effect(() => {
    branchesByProject;
    if (!loading) {
      workspaceLifecycle.scheduleKickoff();
    }
  });

  async function setupBranchWorktree(branchId: string, projectId: string): Promise<void> {
    await workspaceLifecycle.retryWorktree(branchId, projectId);
  }

  function handleDeleteBranchRequest(branchId: string, project: Project) {
    const branches = branchesByProject.get(project.id) || [];
    const branch = branches.find((b) => b.id === branchId);
    if (branch) {
      branchToDelete = { branch, project };
    }
  }

  async function confirmDeleteBranch() {
    if (!branchToDelete) return;
    const { branch } = branchToDelete;
    branchToDelete = null;

    // Show "Deleting…" state on the card immediately
    deletingBranches = new Set([...deletingBranches, branch.id]);

    try {
      if (branch.projectRepoId) {
        await commands.removeProjectRepo(branch.projectId, branch.projectRepoId);
        await projectsDataStore.refreshProject(branch.projectId);
      } else {
        await commands.deleteBranch(branch.id);
        // Fallback for legacy branches without repo linkage
        const existing = branchesByProject.get(branch.projectId) || [];
        projectsDataStore.setBranchesByProject(
          new Map(branchesByProject).set(
            branch.projectId,
            existing.filter((b) => b.id !== branch.id)
          )
        );
      }
      commands.invalidateBranchTimeline(branch.id);
    } catch (e) {
      console.error('Failed to delete branch:', e);
    } finally {
      const next = new Set(deletingBranches);
      next.delete(branch.id);
      deletingBranches = next;
      workspaceLifecycle.clearBranchState(branch.id);
    }
  }

  async function handleRenameBranch(branchId: string, projectId: string, branchName: string) {
    try {
      const updated = await commands.renameBranch(branchId, branchName);
      const existing = branchesByProject.get(projectId) || [];
      projectsDataStore.setBranchesByProject(
        new Map(branchesByProject).set(
          projectId,
          existing.map((b) => (b.id === updated.id ? updated : b))
        )
      );
    } catch (e) {
      console.error('Failed to rename branch:', e);
      throw e;
    }
  }
</script>

<TopBarPortal
  title={selectedProject ? '' : 'Project'}
  center={projectTopBarCenter}
  badges={projectTopBarBadges}
  rightActions={projectTopBarRightActions}
/>

{#snippet projectTopBarCenter()}
  {#if selectedProject && showTopBarProjectName}
    {@const topBarProjectName = projectDisplayName(selectedProject)}
    <span class="top-bar-project-name" title={topBarProjectName}>{topBarProjectName}</span>
  {/if}
{/snippet}

{#snippet projectTopBarBadges()}
  {@const workspaceStatus = getRemoteWorkspaceStatus(selectedProject)}
  {@const workstationName = getRemoteWorkstationName(selectedProject)}

  {#if selectedProjectDeleting}
    <span class="top-bar-badge" role="status" aria-live="polite">
      <Spinner size={12} />
      <span>Deleting</span>
    </span>
  {/if}

  {#if selectedProjectDetecting}
    <span class="top-bar-badge">
      <Spinner size={12} />
      <span>Detecting actions</span>
    </span>
  {/if}

  {#if workspaceStatus}
    <span
      class="top-bar-badge workspace-badge"
      class:starting={workspaceStatus === 'starting'}
      class:running={workspaceStatus === 'running'}
      class:stopped={workspaceStatus === 'stopped'}
      class:suspended={workspaceStatus === 'suspended'}
      class:error={workspaceStatus === 'error'}
      title={workspaceStatus === 'running' && workstationName ? workstationName : undefined}
    >
      {#if workspaceStatus === 'starting'}
        <Spinner size={12} />
      {:else if workspaceStatus === 'running'}
        <Cloud size={12} />
      {:else if workspaceStatus === 'stopped'}
        <CirclePause size={12} />
      {:else if workspaceStatus === 'suspended'}
        <Pause size={12} />
      {:else if workspaceStatus === 'error'}
        <AlertCircle size={12} />
      {/if}
      <span>{statusLabel(workspaceStatus)}</span>
      {#if workspaceStatus === 'suspended' && workstationName && selectedProject}
        <Button
          variant="ghost"
          class="ml-1 h-auto rounded-none border-l border-[var(--border-muted)] bg-transparent px-1 py-0 text-[length:calc(var(--size-xs)-1px)] font-semibold text-[var(--ui-accent)] shadow-none focus-visible:ring-0 hover:bg-transparent hover:text-[var(--ui-accent)] hover:underline"
          onclick={() => workspaceLifecycle.resumeWorkspace(selectedProject!.id, workstationName)}
        >
          Resume
        </Button>
      {/if}
    </span>
  {/if}
{/snippet}

{#snippet projectTopBarRightActions()}
  {#if selectedProject && !selectedProjectDeleting}
    <span class="inline-flex" title={selectedProjectAddRepoHint ?? 'Add repo'}>
      <Button
        variant="ghost"
        size="sm"
        class="top-bar-action group gap-1.5 text-foreground hover:bg-[var(--ui-selection)] hover:text-foreground max-md:size-10 max-md:p-0"
        onclick={() => {
          showAddRepoModal = true;
        }}
        disabled={selectedProjectAddRepoDisabled}
      >
        <span
          class="flex size-4 shrink-0 items-center justify-center text-muted-foreground transition-colors group-hover:not-disabled:text-foreground"
        >
          <Plus size={12} />
        </span>
        <span class="top-bar-action-label">Add Repo</span>
      </Button>
    </span>

    <Button
      variant="ghost"
      size="sm"
      class={[
        'top-bar-action group gap-1.5 text-foreground hover:bg-[var(--ui-selection)] hover:text-destructive max-md:size-10 max-md:p-0',
        selectedProjectSafeToDelete && 'border border-destructive text-destructive',
      ]}
      title="Remove project"
      onclick={() => projectActions.requestRemoveProject(selectedProject)}
    >
      <span
        class={[
          'flex shrink-0 items-center transition-colors',
          selectedProjectSafeToDelete
            ? 'text-destructive'
            : 'text-muted-foreground group-hover:text-destructive',
        ]}
      >
        <Trash2 size={14} />
      </span>
      <span class="top-bar-action-label">Remove Project</span>
    </Button>
  {/if}
{/snippet}

<!-- The projects sidebar renders alongside this view from App.svelte. -->
<div class="project-home">
  <div
    class="main-panel"
    class:no-pad={!loading && !hasContent}
    bind:this={mainPanelEl}
    onscroll={updateTopBarProjectNameVisibility}
  >
    {#if storeIncompat && storeIncompat.kind === 'needs_reset'}
      <div class="update-state">
        <div class="update-card">
          <div class="update-header">
            <h1 class="update-title">Update Required</h1>
            <span class="version-badge new">v{storeIncompat.appVersion}</span>
          </div>
          <p>
            Staged beta updates can require backwards-incompatible changes. The info stored by
            Staged (session history, notes) will be cleared, but your
            <strong>git repos and branches are not affected</strong>.
          </p>
          <div class="update-footer">
            <p class="version-hint">
              Not ready? Install <code>v{storeIncompat.dbAppVersion}</code> instead.
            </p>
            <div class="update-actions">
              <Button variant="ghost" size="sm" onclick={handleClose}>Close</Button>
              <Button variant="outline" size="sm" onclick={handleResetStore} disabled={resetting}>
                {resetting ? 'Resetting…' : 'Reset & Update'}
              </Button>
            </div>
          </div>
        </div>
      </div>
    {:else if storeIncompat && storeIncompat.kind === 'too_new'}
      <div class="update-state">
        <div class="update-card">
          <div class="update-header">
            <h1 class="update-title">Update Staged</h1>
            <span class="version-badge new">v{storeIncompat.dbAppVersion}</span>
          </div>
          <p>
            This database was last used by a newer version of Staged. Please install
            <strong>v{storeIncompat.dbAppVersion}</strong> or newer to continue.
          </p>
          <div class="update-footer">
            <div></div>
            <div class="update-actions">
              <Button variant="ghost" size="sm" onclick={handleClose}>Close</Button>
            </div>
          </div>
        </div>
      </div>
    {:else if error}
      <div class="error-state">
        <p>{error}</p>
      </div>
    {:else if !loading && !hasContent}
      <SplashScreen
        onCreated={handleProjectCreated}
        requestOpen={showNewProjectModal && !hasContent}
        onFormOpenChange={(open) => (showNewProjectModal = open)}
      />
    {:else}
      <div class="projects-list">
        {#each visibleProjects as project (project.id)}
          <ProjectSection
            {project}
            branches={branchesByProject.get(project.id) || []}
            {reposById}
            deleting={deletingProjectNames.has(project.id)}
            {deletingBranches}
            {worktreeErrors}
            {workspaceErrors}
            onDeleteBranch={(branchId) => handleDeleteBranchRequest(branchId, project)}
            onRenameBranch={(branchId, branchName) =>
              handleRenameBranch(branchId, project.id, branchName)}
            onProjectTitleElement={selectedProjectId ? setProjectTitleElement : undefined}
            onRepoSelected={(selection) => handleRepoSelected(project.id, selection)}
            onRetryWorktree={(branchId) => setupBranchWorktree(branchId, project.id)}
          />
        {/each}
      </div>
    {/if}
  </div>
</div>

<!-- New project modal (only when projects exist; splash screen handles inline form otherwise) -->
<NewProjectModal
  open={showNewProjectModal && hasContent}
  onCreated={handleProjectCreated}
  onClose={() => (showNewProjectModal = false)}
/>

<AddRepoModal
  open={showAddRepoModal && !!selectedProject}
  excludeRepos={selectedProjectExcludeRepos}
  onAdded={handleTopBarRepoSelected}
  onClose={() => {
    showAddRepoModal = false;
  }}
/>

<!-- Delete branch confirmation -->
<AlertDialog.Root
  open={branchToDelete !== null}
  onOpenChange={(v) => !v && (branchToDelete = null)}
>
  <AlertDialog.Content>
    {#if branchToDelete}
      <AlertDialog.Header>
        <AlertDialog.Title>Delete Repo</AlertDialog.Title>
        <AlertDialog.Description>
          {`Delete repo for branch "${branchToDelete.branch.branchName}"? This removes its tracked branch and local worktree/remote workspace.`}
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action variant="destructive" onclick={confirmDeleteBranch}>
          Delete
        </AlertDialog.Action>
      </AlertDialog.Footer>
    {/if}
  </AlertDialog.Content>
</AlertDialog.Root>

<style>
  .project-home {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    background-color: var(--bg-chrome);
    overflow: hidden;
  }

  .top-bar-badge {
    height: 22px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 10px;
    border-radius: 999px;
    border: 1px solid var(--border-muted);
    background-color: var(--bg-primary);
    color: var(--text-primary);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
    line-height: 1;
    white-space: nowrap;
  }

  .top-bar-project-name {
    display: inline-block;
    max-width: 100%;
    min-width: 0;
    overflow: hidden;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 650;
    line-height: 1.2;
    text-align: center;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .workspace-badge.starting {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
  }

  .workspace-badge.running {
    border-color: var(--border-muted);
    color: var(--text-primary);
  }

  .workspace-badge.stopped,
  .workspace-badge.suspended {
    border-color: var(--border-muted);
    color: var(--text-muted);
  }

  .workspace-badge.error {
    border-color: var(--ui-danger);
    color: var(--ui-danger);
  }

  :global(.top-bar-action) {
    height: 28px;
    min-width: 0;
  }

  @media (max-width: 768px) {
    .top-bar-action-label {
      display: none;
    }
  }

  .main-panel {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: auto;
    /* Reserve the scrollbar gutter so the centered column never re-centers
       when the vertical scrollbar appears/disappears (avoids a ~4px jog). */
    scrollbar-gutter: stable;
    display: flex;
    flex-direction: column;
  }

  .main-panel.no-pad {
    padding: 0;
  }

  .error-state {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: 1;
    color: var(--text-muted);
  }

  .error-state {
    color: var(--ui-danger);
  }

  /* Store update state */
  .update-state {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: 1;
  }

  .update-card {
    width: 460px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .update-header {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }

  .update-title {
    margin: 0;
    font-size: 22px;
    font-weight: 700;
    color: var(--text-primary);
    letter-spacing: -0.03em;
  }

  .version-badge.new {
    font-family: 'SF Mono', 'Menlo', monospace;
    font-size: var(--size-xs);
    font-weight: 600;
    padding: 2px 7px;
    border-radius: 4px;
    background-color: rgba(63, 185, 80, 0.12);
    color: var(--ui-accent);
  }

  .update-card > p {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
    line-height: 1.6;
  }

  .update-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  .version-hint {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .version-hint code {
    font-family: 'SF Mono', 'Menlo', monospace;
    font-size: var(--size-xs);
    padding: 1px 5px;
    background-color: var(--bg-elevated);
    border-radius: 3px;
    color: var(--text-muted);
  }

  .update-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
  }

  /* Projects list */
  .projects-list {
    width: 100%;
    max-width: 900px;
    margin: 0 auto;
    padding: 16px;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: 32px;
  }

  @media (max-width: 720px) {
    .update-state {
      padding: 16px;
    }

    .update-card {
      width: 100%;
      max-width: 460px;
    }

    .update-footer {
      align-items: stretch;
      flex-direction: column;
    }

    .update-actions {
      justify-content: flex-end;
    }

    .update-actions :global(button) {
      min-height: 40px;
    }

    .projects-list {
      padding: 12px;
      gap: 20px;
    }
  }
</style>
