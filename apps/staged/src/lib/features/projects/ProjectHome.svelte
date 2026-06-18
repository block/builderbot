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
  import PanelLeftClose from '@lucide/svelte/icons/panel-left-close';
  import PanelLeftOpen from '@lucide/svelte/icons/panel-left-open';
  import Pause from '@lucide/svelte/icons/pause';
  import Plus from '@lucide/svelte/icons/plus';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import { getWindowSync, listenToEvent } from '../../transport';
  import type {
    Project,
    ProjectRepo,
    Branch,
    WorkspaceStatus,
    PrStatusChangedEvent,
    SessionStatusPayload,
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
  import ProjectsSidebar from './ProjectsSidebar.svelte';
  import SplashScreen from './SplashScreen.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { Button } from '$lib/components/ui/button';
  import { toast } from 'svelte-sonner';
  import {
    projectsSidebarState,
    setProjects,
    setProjectsSidebarCollapsed,
  } from './projectsSidebarState.svelte';
  import { viewport } from '../../shared/viewport.svelte';
  import { workspaceLifecycle } from './workspaceLifecycle.svelte';
  import { projectRunActionsStore } from '../../stores/projectRunActions.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import {
    canDeleteProjectWithoutConfirmation,
    computeSafeToDeleteSignature,
  } from './projectDeleteSafety';

  /**
   * Merge incoming branches with existing ones, preserving worktreePath when
   * a stale async response would overwrite an already-populated value with null.
   */
  function mergeBranchesPreservingWorktree(existing: Branch[], incoming: Branch[]): Branch[] {
    return incoming.map((newBranch) => {
      const prev = existing.find((b) => b.id === newBranch.id);
      if (prev?.worktreePath && !newBranch.worktreePath) {
        return { ...newBranch, worktreePath: prev.worktreePath };
      }
      return newBranch;
    });
  }

  interface Props {
    selectedProjectId?: string | null;
  }

  let { selectedProjectId = null }: Props = $props();

  // Data
  let projects = $state<Project[]>([]);
  let branchesByProject = $state<Map<string, Branch[]>>(new Map());
  let reposById = $state<Map<string, ProjectRepo>>(new Map());

  /** Replace all cached repos for a single project with a fresh list. */
  function replaceProjectRepos(projectId: string, repos: ProjectRepo[]) {
    const next = new Map(reposById);
    for (const [id, repo] of next) {
      if (repo.projectId === projectId) next.delete(id);
    }
    for (const repo of repos) next.set(repo.id, repo);
    reposById = next;
  }
  let loading = $state(true);
  let error = $state<string | null>(null);
  let loadGeneration = 0;
  let initialLoadComplete = $state(false);
  let lastSelectedProjectId: string | null = null;
  let backgroundHydrationCancel: (() => void) | null = null;
  let queuedSessionDrainCancel: (() => void) | null = null;
  const queuedSessionDrainBranchIds = new Set<string>();

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

  // Delete confirmation state
  let projectToDelete = $state<Project | null>(null);
  let branchToDelete = $state<{ branch: Branch; project: Project } | null>(null);
  let deletingBranches = $state<Set<string>>(new Set());
  let deletingProjectNames = $state<Map<string, string>>(new Map());

  // Setup errors come from the shared workspace lifecycle orchestrator.
  let worktreeErrors = $derived(workspaceLifecycle.getWorktreeErrors());
  let workspaceErrors = $derived(workspaceLifecycle.getWorkspaceErrors());

  // Action detection state
  let detectingProjectIds = $state<Set<string>>(new Set());

  onMount(() => {
    workspaceLifecycle.start({
      getBranchesByProject: () => branchesByProject,
      setBranchesByProject: (next) => {
        branchesByProject = next;
      },
      isProjectDeleting: (projectId) => deletingProjectNames.has(projectId),
    });
    checkStoreAndLoad();
    void projectRunActionsStore.startListening();

    const onNewProject = () => handleNewProject();
    window.addEventListener('staged:new-project', onNewProject);

    const unlistenDetection = listenToRepoActionsDetection((event) => {
      const matchingProjectIds = projects
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

    // Listen for backend-driven setup progress events. The backend emits this
    // after repo creation, after worktree setup, and after prerun actions.
    // We only refresh display state here — setup itself is owned by the backend.
    const unlistenProjectRepoAdded = listenToEvent<string>(
      'project-setup-progress',
      async (projectId) => {
        console.log('[ProjectHome] project-setup-progress event for project', projectId);
        try {
          const [projectsList, branches, repos] = await Promise.all([
            commands.listProjects(),
            commands.listBranchesForProject(projectId),
            commands.listProjectRepos(projectId),
          ]);
          setProjects(projectsList);
          projects = projectsList;
          const mergedBranches = mergeBranchesPreservingWorktree(
            branchesByProject.get(projectId) || [],
            branches
          );
          branchesByProject = new Map(branchesByProject).set(projectId, mergedBranches);
          commands.invalidateProjectBranchTimelines(mergedBranches.map((b) => b.id));
          workspaceLifecycle.enqueueInitialSetup(projectId, mergedBranches);
          replaceProjectRepos(projectId, repos);
          void repoBadgeStore.ensureForRepos(
            repos.map((r) => ({ githubRepo: r.githubRepo, subpath: r.subpath }))
          );
        } catch (e) {
          console.error('[ProjectHome] Failed to refresh project after setup progress:', e);
        }
      }
    );

    // Listen for PR status changes to update branch state.
    //
    // A PR-polling cycle emits one `pr-status-changed` per branch, so a storm
    // of N branches arrives as N separate events. Rebuilding `branchesByProject`
    // with a fresh `new Map(...)` per event means N allocations + N derivation
    // re-runs, which can pile up on the main thread during a project switch.
    // Buffer the events and apply a single rebuild per frame so a burst
    // coalesces into one reactive flush without dropping any update.
    let pendingPrStatusEvents: PrStatusChangedEvent[] = [];
    let prStatusFlushHandle: number | null = null;

    const flushPrStatusEvents = () => {
      prStatusFlushHandle = null;
      if (pendingPrStatusEvents.length === 0) return;
      const events = pendingPrStatusEvents;
      pendingPrStatusEvents = [];

      // Apply every buffered event onto one fresh Map. Each event re-scans the
      // in-progress map, so multiple updates to the same project compound
      // correctly instead of clobbering one another.
      const next = new Map(branchesByProject);
      for (const payload of events) {
        for (const [projectId, branches] of next) {
          const branchIndex = branches.findIndex((b) => b.id === payload.branchId);
          if (branchIndex !== -1) {
            const updatedBranches = [...branches];
            updatedBranches[branchIndex] = {
              ...updatedBranches[branchIndex],
              prState: payload.prState,
              prChecksStatus: payload.prChecksStatus,
              prReviewDecision: payload.prReviewDecision,
              prMergeable: payload.prMergeable,
              prDraft: payload.prDraft,
              prHeadSha: payload.prHeadSha,
              prFetchedAt: payload.prFetchedAt,
            };
            next.set(projectId, updatedBranches);
            break;
          }
        }
      }
      branchesByProject = next;
    };

    const unlistenPrStatus = listenToEvent<PrStatusChangedEvent>('pr-status-changed', (payload) => {
      pendingPrStatusEvents.push(payload);
      if (prStatusFlushHandle === null) {
        prStatusFlushHandle = requestAnimationFrame(flushPrStatusEvents);
      }
    });

    // Refresh a project's branches when a commit session completes so the
    // sprout/draft-PR icon flips as soon as the first commit lands.
    const unlistenSessionStatus = listenToEvent<SessionStatusPayload>(
      'session-status-changed',
      async (payload) => {
        if (payload.status !== 'completed') return;
        if (payload.sessionType !== 'commit') return;
        const projectId = payload.projectId;
        if (!projectId || !branchesByProject.has(projectId)) return;
        try {
          const branches = await commands.listBranchesForProject(projectId);
          branchesByProject = new Map(branchesByProject).set(projectId, branches);
        } catch (e) {
          console.error(`Failed to refresh branches for project ${projectId} after commit:`, e);
        }
      }
    );

    return () => {
      loadGeneration++;
      window.removeEventListener('staged:new-project', onNewProject);
      unlistenDetection();
      unlistenProjectRepoAdded();
      unlistenPrStatus();
      if (prStatusFlushHandle !== null) {
        cancelAnimationFrame(prStatusFlushHandle);
        prStatusFlushHandle = null;
      }
      pendingPrStatusEvents = [];
      unlistenSessionStatus();
      cancelBackgroundHydration();
      cancelQueuedSessionDrain();
      workspaceLifecycle.stop();
      projectRunActionsStore.stopListening();
    };
  });

  async function checkStoreAndLoad() {
    loading = true;
    try {
      const status = await commands.getStoreStatus();
      if (status) {
        storeIncompat = status;
        loading = false;
        return;
      }
      await loadData();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      loading = false;
    }
  }

  async function handleResetStore() {
    resetting = true;
    try {
      await commands.confirmResetStore();
      storeIncompat = null;
      await loadData();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
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

  function cancelBackgroundHydration() {
    backgroundHydrationCancel?.();
    backgroundHydrationCancel = null;
  }

  function cancelQueuedSessionDrain() {
    queuedSessionDrainCancel?.();
    queuedSessionDrainCancel = null;
    queuedSessionDrainBranchIds.clear();
  }

  function scheduleQueuedSessionDrain(branches: Branch[]) {
    for (const branch of branches) {
      const isLocalReady = branch.branchType === 'local' && branch.worktreePath;
      const isRemoteReady = branch.branchType === 'remote' && branch.workspaceStatus === 'running';
      if (isLocalReady || isRemoteReady) {
        queuedSessionDrainBranchIds.add(branch.id);
      }
    }

    if (queuedSessionDrainBranchIds.size === 0 || queuedSessionDrainCancel) return;

    queuedSessionDrainCancel = scheduleDeferredTask(() => {
      queuedSessionDrainCancel = null;
      const branchIds = Array.from(queuedSessionDrainBranchIds);
      queuedSessionDrainBranchIds.clear();
      for (const branchId of branchIds) {
        commands.drainQueuedSessions(branchId).catch((e) => {
          console.error('[ProjectHome] Failed to drain queued sessions on startup:', e);
        });
      }
    }, 3000);
  }

  async function hydrateProject(
    project: Project,
    generation: number,
    options: { drainQueuedSessions?: boolean } = {}
  ): Promise<Branch[] | null> {
    const [branches, repos] = await Promise.all([
      commands.listBranchesForProject(project.id),
      commands.listProjectRepos(project.id),
    ]);
    if (generation !== loadGeneration) return null;

    const mergedBranches = mergeBranchesPreservingWorktree(
      branchesByProject.get(project.id) || [],
      branches
    );
    branchesByProject = new Map(branchesByProject).set(project.id, mergedBranches);
    workspaceLifecycle.enqueueInitialSetup(project.id, mergedBranches);
    replaceProjectRepos(project.id, repos);
    void repoBadgeStore.ensureForRepos(
      repos.map((r) => ({ githubRepo: r.githubRepo, subpath: r.subpath }))
    );
    projectRunActionsStore
      .hydrateFromProjectBranches(branchesByProject, {
        branchIds: mergedBranches.map((b) => b.id),
      })
      .catch(console.error);

    if (options.drainQueuedSessions) {
      scheduleQueuedSessionDrain(mergedBranches);
    }

    return mergedBranches;
  }

  async function hydrateAllProjects(projectList: Project[], generation: number) {
    await Promise.all(
      projectList.map(async (project) => {
        try {
          await hydrateProject(project, generation, { drainQueuedSessions: true });
        } catch (e) {
          console.error(`[ProjectHome] Failed to hydrate project '${project.id}':`, e);
        }
      })
    );
  }

  function scheduleBackgroundHydration(
    projectList: Project[],
    foregroundProjectId: string | null,
    generation: number
  ) {
    cancelBackgroundHydration();

    const queue = projectList.filter((project) => project.id !== foregroundProjectId);
    if (queue.length === 0) return;

    let cancelled = false;
    let cancelScheduledTask: (() => void) | null = null;

    const hydrateNext = () => {
      cancelScheduledTask = null;
      if (cancelled || generation !== loadGeneration) return;

      const project = queue.shift();
      if (!project) return;

      hydrateProject(project, generation, { drainQueuedSessions: true })
        .catch((e) => {
          console.error(`[ProjectHome] Failed to background hydrate project '${project.id}':`, e);
        })
        .finally(() => {
          if (cancelled || generation !== loadGeneration || queue.length === 0) return;
          cancelScheduledTask = scheduleDeferredTask(hydrateNext, 3000);
        });
    };

    cancelScheduledTask = scheduleDeferredTask(hydrateNext, 3000);
    backgroundHydrationCancel = () => {
      cancelled = true;
      cancelScheduledTask?.();
    };
  }

  async function hydrateActionDetection(projectList: Project[], generation: number) {
    try {
      const contexts = await commands.listActionContexts();
      if (generation !== loadGeneration) return;
      detectingProjectIds = new Set(
        projectList
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

  async function hydrateForCurrentSelection(projectId: string | null) {
    const generation = ++loadGeneration;
    cancelBackgroundHydration();
    error = null;

    const projectList = projects;
    if (projectId) {
      const project = projectList.find((p) => p.id === projectId);
      if (!project) return;
      try {
        await hydrateProject(project, generation, { drainQueuedSessions: true });
      } catch (e) {
        if (generation !== loadGeneration) return;
        console.error(`[ProjectHome] Failed to hydrate selected project '${project.id}':`, e);
      }
      if (generation !== loadGeneration) return;
      scheduleBackgroundHydration(projectList, projectId, generation);
    } else {
      await hydrateAllProjects(projectList, generation);
    }

    void hydrateActionDetection(projectList, generation);
  }

  async function loadData() {
    const generation = ++loadGeneration;
    initialLoadComplete = false;
    cancelBackgroundHydration();
    if (projects.length === 0) {
      loading = true;
    }
    error = null;
    await repoBadgeStore.loadAll();
    try {
      const projectList = await commands.listProjects();
      if (generation !== loadGeneration) return;
      projects = projectList;
      setProjects(projectList);
      loading = false;

      // Seed maps so project sections can render immediately.
      const branchMap = new Map<string, Branch[]>();
      for (const project of projectList) {
        branchMap.set(project.id, branchesByProject.get(project.id) || []);
      }
      branchesByProject = branchMap;

      // Drop cached repos for projects that no longer exist.
      const projectIds = new Set(projectList.map((p) => p.id));
      const prunedRepos = new Map<string, ProjectRepo>();
      for (const [id, repo] of reposById) {
        if (projectIds.has(repo.projectId)) prunedRepos.set(id, repo);
      }
      reposById = prunedRepos;

      if (selectedProjectId) {
        const selectedProject = projectList.find((project) => project.id === selectedProjectId);
        if (selectedProject) {
          try {
            await hydrateProject(selectedProject, generation, { drainQueuedSessions: true });
          } catch (e) {
            console.error(
              `[ProjectHome] Failed to hydrate selected project '${selectedProject.id}':`,
              e
            );
          }
        }
        if (generation !== loadGeneration) return;
        scheduleBackgroundHydration(projectList, selectedProjectId, generation);
      } else {
        await hydrateAllProjects(projectList, generation);
      }

      lastSelectedProjectId = selectedProjectId;
      initialLoadComplete = true;
      void hydrateActionDetection(projectList, generation);
    } catch (e) {
      if (generation !== loadGeneration) return;
      error = e instanceof Error ? e.message : String(e);
    } finally {
      if (generation === loadGeneration) {
        loading = false;
      }
    }
  }

  $effect(() => {
    const projectId = selectedProjectId;
    if (!initialLoadComplete) return;
    if (projectId === lastSelectedProjectId) return;
    lastSelectedProjectId = projectId;
    void hydrateForCurrentSelection(projectId);
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
  let repoCountsByProject = $derived(
    new Map(
      projects.map((project) => {
        let knownCount = 0;
        for (const repo of reposById.values()) {
          if (repo.projectId === project.id) knownCount++;
        }
        const fallbackCount = project.githubRepo ? 1 : 0;
        return [project.id, knownCount > 0 ? knownCount : fallbackCount] as const;
      })
    )
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
          [...reposById.values()]
            .filter((repo) => repo.projectId === selectedProject.id)
            .map((repo) => `${repo.githubRepo}\x00${repo.subpath ?? ''}`)
        )
      : new Set<string>()
  );
  let sidebarOpen = $derived(!projectsSidebarState.collapsed);

  let reposByProject = $derived(
    new Map(
      projects.map((project) => {
        const repos: ProjectRepo[] = [];
        for (const repo of reposById.values()) {
          if (repo.projectId === project.id) repos.push(repo);
        }
        return [project.id, repos] as const;
      })
    )
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
  $effect(() => {
    // Read reactive deps synchronously so the effect re-subscribes correctly.
    const projectsSnapshot = visibleProjects;
    const branches = branchesByProject;
    const repoCounts = repoCountsByProject;

    const signature = computeSafeToDeleteSignature(projectsSnapshot, branches, repoCounts);
    if (signature === lastSafeSignature) {
      // Inputs relevant to the result are unchanged — skip the git work.
      return;
    }

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

    // Defer off the critical render path: let the switch's keyed-block swap
    // flush first, then settle the cosmetic styling during idle. The timeout
    // guarantees the check still runs even while the main thread stays busy
    // through the post-switch hydration window (otherwise an idle callback can
    // be deferred indefinitely under sustained load).
    const schedule =
      typeof requestIdleCallback === 'function'
        ? (cb: () => void) => requestIdleCallback(cb, { timeout: 2000 })
        : (cb: () => void) => setTimeout(cb, 0) as unknown as number;
    const cancel =
      typeof cancelIdleCallback === 'function'
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

  function handleMarkProjectUnread(project: Project) {
    if (deletingProjectNames.has(project.id)) return;
    projectStateStore.markAsUnread(project.id);
  }

  async function handleProjectCreated(project: Project) {
    if (!projects.some((p) => p.id === project.id)) {
      projects = [...projects, project];
    }
    showNewProjectModal = false;
    selectProject(project.id);
    // Hydrate branches and repos in the background so the modal closes instantly
    try {
      const [branches, repos] = await Promise.all([
        commands.listBranchesForProject(project.id),
        commands.listProjectRepos(project.id),
      ]);
      branchesByProject = new Map(branchesByProject).set(project.id, branches);
      workspaceLifecycle.enqueueInitialSetup(project.id, branches);
      replaceProjectRepos(project.id, repos);
    } catch (e) {
      console.error('[ProjectHome] Failed to hydrate newly created project:', e);
    }
  }

  async function handleDeleteProjectRequest(project: Project) {
    const branches = branchesByProject.get(project.id) || [];
    const repoCount = repoCountsByProject.get(project.id) || 0;

    const isSafeToDelete = await canDeleteProjectWithoutConfirmation({
      branches,
      repoCount,
      hasUnpushedCommits: commands.hasUnpushedCommits,
      onCheckError: (e) => console.error('Failed to check unpushed commits:', e),
    });

    if (isSafeToDelete) {
      // Safe to delete without confirmation
      projectToDelete = project;
      await confirmDeleteProject();
    } else {
      // Show confirmation dialog
      projectToDelete = project;
    }
  }

  async function confirmDeleteProject() {
    if (!projectToDelete) return;
    const id = projectToDelete.id;
    const name = projectDisplayName(projectToDelete);
    const branchesToClear = branchesByProject.get(id) || [];
    projectToDelete = null;
    deletingProjectNames = new Map(deletingProjectNames).set(id, name);
    window.dispatchEvent(
      new CustomEvent('staged:project-delete-start', {
        detail: { projectId: id, name },
      })
    );

    // Navigate away immediately so the user doesn't have to wait for backend deletion.
    // Skip projects that are already being deleted.
    const currentIndex = projects.findIndex((p) => p.id === id);
    const alive = projects.filter((p) => p.id !== id && !deletingProjectNames.has(p.id));
    if (alive.length > 0) {
      // Prefer the next project after the current one; fall back to the closest earlier one
      const next = alive.find((p) => projects.indexOf(p) > currentIndex) ?? alive[alive.length - 1];
      selectProject(next.id);
    } else {
      goHome();
    }

    try {
      await commands.deleteProject(id);
      projectStateStore.markAsRead(id);
      projects = projects.filter((p) => p.id !== id);
      setProjects(projects);
      const nextBranches = new Map(branchesByProject);
      nextBranches.delete(id);
      branchesByProject = nextBranches;
      const nextRepos = new Map(reposById);
      for (const [repoId, repo] of nextRepos) {
        if (repo.projectId === id) nextRepos.delete(repoId);
      }
      reposById = nextRepos;
      commands.invalidateProjectBranchTimelines(branchesToClear.map((b) => b.id));
      for (const branch of branchesToClear) {
        workspaceLifecycle.clearBranchState(branch.id);
      }
    } catch (e) {
      console.error('Failed to delete project:', e);
      const message = e instanceof Error ? e.message : String(e);
      toast.error('Unable to delete project', { description: message });
    } finally {
      const next = new Map(deletingProjectNames);
      next.delete(id);
      deletingProjectNames = next;
      window.dispatchEvent(
        new CustomEvent('staged:project-delete-end', {
          detail: { projectId: id },
        })
      );
    }
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
      const [projectsList, branches, repos] = await Promise.all([
        commands.listProjects(),
        commands.listBranchesForProject(projectId),
        commands.listProjectRepos(projectId),
      ]);
      setProjects(projectsList);
      projects = projectsList;
      const mergedBranches = mergeBranchesPreservingWorktree(
        branchesByProject.get(projectId) || [],
        branches
      );
      branchesByProject = new Map(branchesByProject).set(projectId, mergedBranches);
      commands.invalidateProjectBranchTimelines(mergedBranches.map((b) => b.id));
      workspaceLifecycle.enqueueInitialSetup(projectId, mergedBranches);
      replaceProjectRepos(projectId, repos);
      void repoBadgeStore.ensureForRepos(
        repos.map((r) => ({ githubRepo: r.githubRepo, subpath: r.subpath }))
      );
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

  function toggleProjectsSidebar() {
    setProjectsSidebarCollapsed(!projectsSidebarState.collapsed);
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
        const [projectsList, branches, repos] = await Promise.all([
          commands.listProjects(),
          commands.listBranchesForProject(branch.projectId),
          commands.listProjectRepos(branch.projectId),
        ]);
        setProjects(projectsList);
        projects = projectsList;
        branchesByProject = new Map(branchesByProject).set(branch.projectId, branches);
        replaceProjectRepos(branch.projectId, repos);
      } else {
        await commands.deleteBranch(branch.id);
        // Fallback for legacy branches without repo linkage
        const existing = branchesByProject.get(branch.projectId) || [];
        branchesByProject = new Map(branchesByProject).set(
          branch.projectId,
          existing.filter((b) => b.id !== branch.id)
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
      branchesByProject = new Map(branchesByProject).set(
        projectId,
        existing.map((b) => (b.id === updated.id ? updated : b))
      );
    } catch (e) {
      console.error('Failed to rename branch:', e);
    }
  }
</script>

<TopBarPortal
  title={selectedProject ? '' : 'Project'}
  leftActions={projectTopBarLeftActions}
  center={projectTopBarCenter}
  badges={projectTopBarBadges}
  rightActions={projectTopBarRightActions}
/>

{#snippet projectTopBarLeftActions()}
  {#if !viewport.isMobile}
    <span
      class="inline-flex"
      title={sidebarOpen ? 'Hide projects sidebar' : 'Show projects sidebar'}
    >
      <Button
        variant="ghost"
        size="sm"
        class="top-bar-action gap-1.5 text-foreground hover:bg-[var(--ui-selection)] hover:text-foreground max-md:size-10 max-md:p-0 [&_svg]:size-3.5"
        aria-label={sidebarOpen ? 'Hide projects sidebar' : 'Show projects sidebar'}
        onclick={toggleProjectsSidebar}
        disabled={!projectsSidebarState.hasProjects}
      >
        {#if !sidebarOpen || !projectsSidebarState.hasProjects}
          <PanelLeftOpen size={14} />
          <span class="top-bar-action-label">Show Sidebar</span>
        {:else}
          <PanelLeftClose size={14} />
          <span class="top-bar-action-label">Hide Sidebar</span>
        {/if}
      </Button>
    </span>
  {/if}
{/snippet}

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
      onclick={() => handleDeleteProjectRequest(selectedProject)}
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

<div class="project-home">
  <ProjectsSidebar
    {projects}
    {loading}
    {error}
    {deletingProjectNames}
    {repoCountsByProject}
    {reposByProject}
    projectBranches={branchesByProject}
    showAllProjectsRow={true}
    onMarkProjectUnread={handleMarkProjectUnread}
    onRemoveProject={handleDeleteProjectRequest}
  />

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

<!-- Delete project confirmation -->
<AlertDialog.Root
  open={projectToDelete !== null}
  onOpenChange={(v) => !v && (projectToDelete = null)}
>
  <AlertDialog.Content>
    {#if projectToDelete}
      <AlertDialog.Header>
        <AlertDialog.Title>Remove Project</AlertDialog.Title>
        <AlertDialog.Description>
          {`Remove "${projectDisplayName(projectToDelete)}" from Staged? There are unmerged changes in this project's branches. Deleting this project will lose any changes not pushed to GitHub.`}
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action variant="destructive" onclick={confirmDeleteProject}>
          Remove
        </AlertDialog.Action>
      </AlertDialog.Footer>
    {/if}
  </AlertDialog.Content>
</AlertDialog.Root>

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
    padding: 16px 24px 24px;
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
      padding: 12px 16px 16px;
      gap: 20px;
    }
  }
</style>
