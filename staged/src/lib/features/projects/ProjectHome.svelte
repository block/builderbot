<!--
  ProjectHome.svelte - Project workspace page

  In app navigation this is the "project page". It can render a single selected
  project (detail view) or multiple projects when no filter is provided.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import type { Project, Branch, StoreIncompatibility, WorkspaceStatus } from '../../types';
  import * as commands from '../../commands';
  import { listenToRepoActionsDetection, runPrerunActions } from '../actions/actions';
  import { projectDisplayName } from '../../shared/utils';
  import { goHome, selectProject } from '../../navigation.svelte';
  import ProjectSection from './ProjectSection.svelte';
  import NewProjectModal from './NewProjectModal.svelte';
  import ProjectsSidebar from './ProjectsSidebar.svelte';
  import GitHubRepoPickerModal from './GitHubRepoPickerModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import SplashScreen from './SplashScreen.svelte';
  import { alerts } from '../../shared/alerts.svelte';
  import { setHasProjects } from './projectsSidebarState.svelte';

  interface Props {
    selectedProjectId?: string | null;
  }

  let { selectedProjectId = null }: Props = $props();

  // Data
  let projects = $state<Project[]>([]);
  let branchesByProject = $state<Map<string, Branch[]>>(new Map());
  let repoLabelsByProject = $state<Map<string, Map<string, string>>>(new Map());
  let loading = $state(true);
  let error = $state<string | null>(null);
  let loadGeneration = 0;

  // Store health — if non-null the DB needs a reset before we can proceed
  let storeIncompat = $state<StoreIncompatibility | null>(null);
  let resetting = $state(false);

  // Modal state
  let showNewProjectModal = $state(false);
  let showRepoPicker = $state(false);
  let repoPickerProject = $state<Project | null>(null);

  // Delete confirmation state
  let projectToDelete = $state<Project | null>(null);
  let branchToDelete = $state<{ branch: Branch; project: Project } | null>(null);
  let deletingBranches = $state<Set<string>>(new Set());
  let deletingProjectNames = $state<Map<string, string>>(new Map());

  // Worktree setup errors — maps branch ID → error message
  let worktreeErrors = $state<Map<string, string>>(new Map());
  let pendingSetupBranches = $state<Set<string>>(new Set());
  let queuedSetupBranches = $state<Set<string>>(new Set());
  let activeSetupCount = 0;
  const MAX_SETUP_CONCURRENCY = 1;
  const setupTaskQueue: Array<{ branchId: string; run: () => Promise<void> }> = [];
  let kickoffTimer: ReturnType<typeof setTimeout> | null = null;

  // Action detection state
  let detectingProjectIds = $state<Set<string>>(new Set());

  onMount(() => {
    checkStoreAndLoad();

    const onNewProject = () => handleNewProject();
    window.addEventListener('staged:new-project', onNewProject);

    let unlistenDetection: (() => void) | undefined;
    listenToRepoActionsDetection((event) => {
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
    }).then((unlisten) => {
      unlistenDetection = unlisten;
    });

    // Listen for PR status changes to update branch state
    let unlistenPrStatus: UnlistenFn | undefined;
    listen<{
      branchId: string;
      prState: string;
      prChecksStatus: string;
      prReviewDecision: string | null;
      prMergeable: boolean;
      prDraft: boolean;
    }>('pr-status-changed', (event) => {
      const payload = event.payload;
      // Find the project that contains this branch and update it
      for (const [projectId, branches] of branchesByProject.entries()) {
        const branchIndex = branches.findIndex((b) => b.id === payload.branchId);
        if (branchIndex !== -1) {
          // Update the branch with new PR status
          const updatedBranches = [...branches];
          updatedBranches[branchIndex] = {
            ...updatedBranches[branchIndex],
            prState: payload.prState,
            prChecksStatus: payload.prChecksStatus,
            prReviewDecision: payload.prReviewDecision,
            prMergeable: payload.prMergeable,
            prDraft: payload.prDraft,
          };
          branchesByProject = new Map(branchesByProject).set(projectId, updatedBranches);
          break;
        }
      }
    }).then((unlisten) => {
      unlistenPrStatus = unlisten;
    });

    return () => {
      window.removeEventListener('staged:new-project', onNewProject);
      unlistenDetection?.();
      unlistenPrStatus?.();
      if (kickoffTimer) {
        clearTimeout(kickoffTimer);
        kickoffTimer = null;
      }
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
    getCurrentWindow().close();
  }

  async function loadData() {
    const generation = ++loadGeneration;
    if (projects.length === 0) {
      loading = true;
    }
    error = null;
    try {
      const projectList = await commands.listProjects();
      if (generation !== loadGeneration) return;
      projects = projectList;
      setHasProjects(projectList.length > 0);
      loading = false;

      // Seed maps so project sections can render immediately.
      const branchMap = new Map<string, Branch[]>();
      const repoLabelMap = new Map<string, Map<string, string>>();
      for (const project of projectList) {
        branchMap.set(project.id, branchesByProject.get(project.id) || []);
        repoLabelMap.set(project.id, repoLabelsByProject.get(project.id) || new Map());
      }
      branchesByProject = branchMap;
      repoLabelsByProject = repoLabelMap;

      await Promise.all(
        projectList.map(async (project) => {
          try {
            const [branches, repos] = await Promise.all([
              commands.listBranchesForProject(project.id),
              commands.listProjectRepos(project.id),
            ]);
            if (generation !== loadGeneration) return;
            branchesByProject = new Map(branchesByProject).set(project.id, branches);
            repoLabelsByProject = new Map(repoLabelsByProject).set(
              project.id,
              new Map(repos.map((repo) => [repo.id, repo.githubRepo] as const))
            );
          } catch (e) {
            console.error(`[ProjectHome] Failed to hydrate project '${project.id}':`, e);
          }
        })
      );

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
    } catch (e) {
      if (generation !== loadGeneration) return;
      error = e instanceof Error ? e.message : String(e);
    } finally {
      if (generation === loadGeneration) {
        loading = false;
      }
    }
  }

  let visibleProjects = $derived(
    selectedProjectId ? projects.filter((project) => project.id === selectedProjectId) : projects
  );
  let hasContent = $derived(visibleProjects.length > 0);
  let selectedProject = $derived(
    selectedProjectId ? projects.find((project) => project.id === selectedProjectId) || null : null
  );
  let repoCountsByProject = $derived(
    new Map(
      projects.map((project) => {
        const knownCount = repoLabelsByProject.get(project.id)?.size ?? 0;
        const fallbackCount = project.githubRepo ? 1 : 0;
        return [project.id, knownCount > 0 ? knownCount : fallbackCount] as const;
      })
    )
  );

  // Track which projects are safe to delete (for button styling)
  let safeToDeleteProjects = $state<Set<string>>(new Set());

  // Update safe-to-delete status when branches change
  $effect(() => {
    const updateSafeStatus = async () => {
      const nextSafe = new Set<string>();

      for (const project of projects) {
        const branches = branchesByProject.get(project.id) || [];
        const repoCount = repoCountsByProject.get(project.id) || 0;

        // If no repos, safe to delete
        if (repoCount === 0) {
          nextSafe.add(project.id);
          continue;
        }

        // Check if all branches have merged PRs and no unpushed changes
        if (branches.length > 0) {
          const allSafe = await Promise.all(
            branches.map(async (branch) => {
              const isMerged = branch.prState === 'MERGED';
              if (!isMerged) return false;

              try {
                const hasUnpushed = await commands.hasUnpushedCommits(branch.id);
                return !hasUnpushed;
              } catch (e) {
                return false;
              }
            })
          );

          if (allSafe.every((safe) => safe)) {
            nextSafe.add(project.id);
          }
        }
      }

      safeToDeleteProjects = nextSafe;
    };

    updateSafeStatus();
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

  async function handleProjectCreated(project: Project) {
    if (!projects.some((p) => p.id === project.id)) {
      projects = [...projects, project];
    }
    const [branches, repos] = await Promise.all([
      commands.listBranchesForProject(project.id),
      commands.listProjectRepos(project.id),
    ]);
    branchesByProject = new Map(branchesByProject).set(project.id, branches);
    repoLabelsByProject = new Map(repoLabelsByProject).set(
      project.id,
      new Map(repos.map((repo) => [repo.id, repo.githubRepo] as const))
    );
    startInitialBranchSetup(project.id, branches);
    showNewProjectModal = false;
    selectProject(project.id);
  }

  async function handleDeleteProjectRequest(project: Project) {
    const branches = branchesByProject.get(project.id) || [];
    const repoCount = repoCountsByProject.get(project.id) || 0;

    // If no repos, safe to delete without confirmation
    if (repoCount === 0) {
      projectToDelete = project;
      // Immediately confirm since it's safe
      await confirmDeleteProject();
      return;
    }

    // Check if all branches have merged PRs and no unpushed changes
    const allSafe = await Promise.all(
      branches.map(async (branch) => {
        const isMerged = branch.prState === 'MERGED';
        if (!isMerged) return false;

        // Check for unpushed commits
        try {
          const hasUnpushed = await commands.hasUnpushedCommits(branch.id);
          return !hasUnpushed;
        } catch (e) {
          console.error('Failed to check unpushed commits:', e);
          return false;
        }
      })
    );

    const isSafeToDelete = branches.length > 0 && allSafe.every((safe) => safe);

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
    projectToDelete = null;
    deletingProjectNames = new Map(deletingProjectNames).set(id, name);
    window.dispatchEvent(
      new CustomEvent('staged:project-delete-start', {
        detail: { projectId: id, name },
      })
    );

    try {
      await commands.deleteProject(id);
      projects = projects.filter((p) => p.id !== id);
      setHasProjects(projects.length > 0);
      const nextBranches = new Map(branchesByProject);
      nextBranches.delete(id);
      branchesByProject = nextBranches;
      const nextRepoLabels = new Map(repoLabelsByProject);
      nextRepoLabels.delete(id);
      repoLabelsByProject = nextRepoLabels;
    } catch (e) {
      console.error('Failed to delete project:', e);
      const message = e instanceof Error ? e.message : String(e);
      alerts.show({
        tone: 'error',
        title: 'Unable to delete project',
        message,
      });
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

  function handleAddRepo(project: Project) {
    if (!canAddRepo(project)) {
      alerts.show({
        tone: 'warning',
        title: 'Unable to add repository',
        message: addRepoHint(project),
      });
      return;
    }
    repoPickerProject = project;
    showRepoPicker = true;
  }

  async function handleRepoSelected(nameWithOwner: string, subpath?: string) {
    if (!repoPickerProject) return;
    try {
      await commands.addProjectRepo(repoPickerProject.id, nameWithOwner, undefined, subpath);
      const [projectsList, branches, repos] = await Promise.all([
        commands.listProjects(),
        commands.listBranchesForProject(repoPickerProject.id),
        commands.listProjectRepos(repoPickerProject.id),
      ]);
      projects = projectsList;
      branchesByProject = new Map(branchesByProject).set(repoPickerProject.id, branches);
      repoLabelsByProject = new Map(repoLabelsByProject).set(
        repoPickerProject.id,
        new Map(repos.map((repo) => [repo.id, repo.githubRepo] as const))
      );
      startInitialBranchSetup(repoPickerProject.id, branches);
    } catch (e) {
      console.error('Failed to add repo:', e);
      const message = e instanceof Error ? e.message : String(e);
      alerts.show({
        tone: 'error',
        title: 'Unable to add repository',
        message,
        durationMs: 0,
      });
    } finally {
      showRepoPicker = false;
      repoPickerProject = null;
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

  function handleWorkspaceStatusChange(
    projectId: string,
    branchId: string,
    workspaceStatus: WorkspaceStatus
  ) {
    const branches = branchesByProject.get(projectId);
    if (!branches) return;
    let changed = false;
    const nextBranches = branches.map((branch) => {
      if (branch.id !== branchId) return branch;
      if (branch.workspaceStatus === workspaceStatus) return branch;
      changed = true;
      return { ...branch, workspaceStatus };
    });
    if (!changed) return;

    branchesByProject = new Map(branchesByProject).set(projectId, nextBranches);
  }

  function startInitialBranchSetup(projectId: string, branches: Branch[]) {
    for (const branch of branches) {
      enqueueBranchSetup(projectId, branch);
    }
  }

  function pumpSetupQueue() {
    while (activeSetupCount < MAX_SETUP_CONCURRENCY && setupTaskQueue.length > 0) {
      const task = setupTaskQueue.shift();
      if (!task) break;

      activeSetupCount += 1;
      task
        .run()
        .catch((e) => {
          console.error('[ProjectHome] Branch setup task failed:', e);
        })
        .finally(() => {
          activeSetupCount = Math.max(0, activeSetupCount - 1);
          const nextQueued = new Set(queuedSetupBranches);
          nextQueued.delete(task.branchId);
          queuedSetupBranches = nextQueued;
          pumpSetupQueue();
        });
    }
  }

  function enqueueBranchSetup(projectId: string, branch: Branch) {
    const branchId = branch.id;
    if (pendingSetupBranches.has(branchId) || queuedSetupBranches.has(branchId)) return;

    if (branch.branchType === 'local') {
      if (branch.worktreePath || worktreeErrors.has(branchId)) return;
      queuedSetupBranches = new Set([...queuedSetupBranches, branchId]);
      setupTaskQueue.push({
        branchId,
        run: async () => {
          await setupBranchWorktree(branchId, projectId);
        },
      });
      pumpSetupQueue();
      return;
    }

    if (branch.branchType === 'remote' && branch.workspaceStatus === 'starting') {
      queuedSetupBranches = new Set([...queuedSetupBranches, branchId]);
      setupTaskQueue.push({
        branchId,
        run: async () => {
          pendingSetupBranches = new Set([...pendingSetupBranches, branchId]);
          try {
            await commands.startWorkspace(branchId);
          } catch (e) {
            console.error('[ProjectHome] Failed to start workspace:', e);
          } finally {
            const next = new Set(pendingSetupBranches);
            next.delete(branchId);
            pendingSetupBranches = next;
          }
        },
      });
      pumpSetupQueue();
    }
  }

  function kickOffPendingBranchSetup(branchMap: Map<string, Branch[]>) {
    for (const [projectId, branches] of branchMap.entries()) {
      if (deletingProjectNames.has(projectId)) continue;
      for (const branch of branches) {
        enqueueBranchSetup(projectId, branch);
      }
    }
  }

  $effect(() => {
    // Ensure pending setup starts even when we navigated to a project page
    // after creation and only loaded persisted branch records.
    if (!loading) {
      if (kickoffTimer) clearTimeout(kickoffTimer);
      kickoffTimer = setTimeout(() => {
        kickoffTimer = null;
        kickOffPendingBranchSetup(branchesByProject);
      }, 50);
    }
  });

  /** Set up a git worktree for a branch, updating the UI on success or error. */
  async function setupBranchWorktree(branchId: string, projectId: string): Promise<void> {
    if (pendingSetupBranches.has(branchId)) return;
    pendingSetupBranches = new Set([...pendingSetupBranches, branchId]);

    // Clear any previous error for this branch
    const nextErrors = new Map(worktreeErrors);
    nextErrors.delete(branchId);
    worktreeErrors = nextErrors;

    try {
      const updated = await commands.setupWorktree(branchId);
      // Replace the branch record so the card picks up worktreePath
      const branches = branchesByProject.get(projectId) || [];
      branchesByProject = new Map(branchesByProject).set(
        projectId,
        branches.map((b) => (b.id === updated.id ? updated : b))
      );

      // Now that the worktree exists, run prerun actions
      setTimeout(() => {
        runPrerunActions(branchId).catch((e) => {
          console.error('[ProjectHome] Failed to run prerun actions:', e);
        });
      }, 150);
    } catch (e) {
      console.error('[ProjectHome] Failed to setup worktree:', e);
      const errMsg = e instanceof Error ? e.message : typeof e === 'string' ? e : String(e);
      worktreeErrors = new Map(worktreeErrors).set(branchId, errMsg);
      throw e;
    } finally {
      const next = new Set(pendingSetupBranches);
      next.delete(branchId);
      pendingSetupBranches = next;
    }
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
        projects = projectsList;
        branchesByProject = new Map(branchesByProject).set(branch.projectId, branches);
        repoLabelsByProject = new Map(repoLabelsByProject).set(
          branch.projectId,
          new Map(repos.map((repo) => [repo.id, repo.githubRepo] as const))
        );
      } else {
        await commands.deleteBranch(branch.id);
        // Fallback for legacy branches without repo linkage
        const existing = branchesByProject.get(branch.projectId) || [];
        branchesByProject = new Map(branchesByProject).set(
          branch.projectId,
          existing.filter((b) => b.id !== branch.id)
        );
      }
    } catch (e) {
      console.error('Failed to delete branch:', e);
    } finally {
      const next = new Set(deletingBranches);
      next.delete(branch.id);
      deletingBranches = next;
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

  // ── Keyboard shortcuts ──

  function handleKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement;
    const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';
    if (isInput && e.key !== 'Escape') return;

    if (e.metaKey && e.key === 'n') {
      e.preventDefault();
      handleNewProject();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="project-home">
  <ProjectsSidebar
    {projects}
    {loading}
    {error}
    {deletingProjectNames}
    {repoCountsByProject}
    projectBranches={branchesByProject}
    showAllProjectsRow={true}
  />

  <div class="main-panel" class:no-pad={!loading && !hasContent}>
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
              <button class="close-button" onclick={handleClose}>Close</button>
              <button class="reset-button" onclick={handleResetStore} disabled={resetting}>
                {resetting ? 'Resetting…' : 'Reset & Update'}
              </button>
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
              <button class="close-button" onclick={handleClose}>Close</button>
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
            repoLabelsById={repoLabelsByProject.get(project.id) || new Map()}
            canAddRepo={canAddRepo(project)}
            addRepoHint={project.location === 'remote' ? addRepoHint(project) : null}
            deleting={deletingProjectNames.has(project.id)}
            safeToDelete={safeToDeleteProjects.has(project.id)}
            {deletingBranches}
            {worktreeErrors}
            detecting={detectingProjectIds.has(project.id)}
            onDeleteProject={() => handleDeleteProjectRequest(project)}
            onDeleteBranch={(branchId) => handleDeleteBranchRequest(branchId, project)}
            onRenameBranch={(branchId, branchName) =>
              handleRenameBranch(branchId, project.id, branchName)}
            onWorkspaceStatusChange={(branchId, workspaceStatus) =>
              handleWorkspaceStatusChange(project.id, branchId, workspaceStatus)}
            onAddRepo={() => handleAddRepo(project)}
            onRetryWorktree={(branchId) => setupBranchWorktree(branchId, project.id)}
          />
        {/each}
      </div>
    {/if}
  </div>
</div>

{#if deletingProjectNames.size > 0}
  <div class="delete-toast" role="status" aria-live="polite">
    {#if deletingProjectNames.size === 1}
      Deleting project “{[...deletingProjectNames.values()][0]}”…
    {:else}
      Deleting {deletingProjectNames.size} projects…
    {/if}
  </div>
{/if}

<!-- New project modal (only when projects exist; splash screen handles inline form otherwise) -->
{#if showNewProjectModal && hasContent}
  <NewProjectModal onCreated={handleProjectCreated} onClose={() => (showNewProjectModal = false)} />
{/if}

{#if showRepoPicker}
  <GitHubRepoPickerModal
    onSelect={handleRepoSelected}
    onClose={() => {
      showRepoPicker = false;
      repoPickerProject = null;
    }}
  />
{/if}

<!-- Delete project confirmation -->
{#if projectToDelete}
  <ConfirmDialog
    title="Remove Project"
    message={`Remove "${projectDisplayName(projectToDelete)}" from Staged? There are unmerged changes in this project's branches. Deleting this project will lose any changes not pushed to GitHub.`}
    confirmLabel="Remove"
    danger={true}
    onConfirm={confirmDeleteProject}
    onCancel={() => (projectToDelete = null)}
  />
{/if}

<!-- Delete branch confirmation -->
{#if branchToDelete}
  <ConfirmDialog
    title="Delete Repo"
    message={`Delete repo for branch "${branchToDelete.branch.branchName}"? This removes its tracked branch and local worktree/remote workspace.`}
    confirmLabel="Delete"
    danger={true}
    onConfirm={confirmDeleteBranch}
    onCancel={() => (branchToDelete = null)}
  />
{/if}

<style>
  .project-home {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    background-color: var(--bg-chrome);
    overflow: hidden;
  }

  .main-panel {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: auto;
    padding: 12px 24px 24px;
    display: flex;
    flex-direction: column;
  }

  .main-panel.no-pad {
    padding: 0;
  }

  .delete-toast {
    position: fixed;
    right: 20px;
    bottom: 20px;
    z-index: 1200;
    background: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    box-shadow: var(--shadow-elevated);
    border-radius: 8px;
    padding: 10px 12px;
    color: var(--text-primary);
    font-size: var(--size-sm);
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

  .close-button {
    padding: 7px 16px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .close-button:hover {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
  }

  .reset-button {
    padding: 7px 16px;
    background-color: var(--ui-accent);
    border: none;
    border-radius: 8px;
    color: var(--bg-deepest);
    font-size: var(--size-sm);
    font-weight: 600;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .reset-button:hover {
    background-color: var(--ui-accent-hover);
  }

  .reset-button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  /* Projects list */
  .projects-list {
    width: 100%;
    max-width: 800px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 32px;
  }
</style>
