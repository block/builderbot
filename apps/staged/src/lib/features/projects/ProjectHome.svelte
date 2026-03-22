<!--
  ProjectHome.svelte - Project workspace page

  In app navigation this is the "project page". It can render a single selected
  project (detail view) or multiple projects when no filter is provided.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import type {
    Project,
    ProjectRepo,
    Branch,
    StoreIncompatibility,
    WorkspaceStatus,
  } from '../../types';
  import * as commands from '../../api/commands';
  import { listenToRepoActionsDetection } from '../actions/actions';
  import { projectDisplayName } from '../../shared/utils';
  import { goHome, selectProject } from '../layout/navigation.svelte';
  import ProjectSection from './ProjectSection.svelte';
  import type { RepoSelection as RepoPickerSelection } from '../../shared/githubUrl';
  import NewProjectModal from './NewProjectModal.svelte';
  import ProjectsSidebar from './ProjectsSidebar.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import SplashScreen from './SplashScreen.svelte';
  import { alerts } from '../../shared/alerts.svelte';
  import { setHasProjects } from './projectsSidebarState.svelte';
  import { workspaceLifecycle } from './workspaceLifecycle.svelte';

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

  // Store health — if non-null the DB needs a reset before we can proceed
  let storeIncompat = $state<StoreIncompatibility | null>(null);
  let resetting = $state(false);

  // Modal state
  let showNewProjectModal = $state(false);

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
      getVisibleProjectIds: () => new Set(visibleProjects.map((project) => project.id)),
      isProjectDeleting: (projectId) => deletingProjectNames.has(projectId),
    });
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

    // Listen for backend-driven setup progress events. The backend emits this
    // after repo creation, after worktree setup, and after prerun actions.
    // We only refresh display state here — setup itself is owned by the backend.
    let unlistenProjectRepoAdded: UnlistenFn | undefined;
    listen<string>('project-setup-progress', async (event) => {
      const projectId = event.payload;
      console.log('[ProjectHome] project-setup-progress event for project', projectId);
      try {
        const [projectsList, branches, repos] = await Promise.all([
          commands.listProjects(),
          commands.listBranchesForProject(projectId),
          commands.listProjectRepos(projectId),
        ]);
        projects = projectsList;
        branchesByProject = new Map(branchesByProject).set(projectId, branches);
        workspaceLifecycle.enqueueInitialSetup(projectId, branches);
        replaceProjectRepos(projectId, repos);
      } catch (e) {
        console.error('[ProjectHome] Failed to refresh project after setup progress:', e);
      }
    }).then((unlisten) => {
      unlistenProjectRepoAdded = unlisten;
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
      unlistenProjectRepoAdded?.();
      unlistenPrStatus?.();
      workspaceLifecycle.stop();
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

      await Promise.all(
        projectList.map(async (project) => {
          try {
            const [branches, repos] = await Promise.all([
              commands.listBranchesForProject(project.id),
              commands.listProjectRepos(project.id),
            ]);
            if (generation !== loadGeneration) return;
            branchesByProject = new Map(branchesByProject).set(project.id, branches);
            workspaceLifecycle.enqueueInitialSetup(project.id, branches);
            replaceProjectRepos(project.id, repos);
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

  // Update safe-to-delete status when branches change.
  // Only check visible projects — calling hasUnpushedCommits for every
  // project wastes IPC round-trips (especially expensive for remote branches)
  // and the result is only consumed in the visibleProjects render loop.
  $effect(() => {
    const updateSafeStatus = async () => {
      const nextSafe = new Set<string>();

      for (const project of visibleProjects) {
        const branches = branchesByProject.get(project.id) || [];
        const repoCount = repoCountsByProject.get(project.id) || 0;

        // Don't show red styling for projects with no repos — there's nothing
        // to call attention to when no repositories have been added yet.
        if (repoCount === 0) {
          continue;
        }

        // Check if all branches have merged PRs and no unpushed changes.
        // Skip the expensive hasUnpushedCommits check for remote branches —
        // their commits live on the workspace and the SSH round-trip (~5s)
        // blocks the UI. Treat merged remote branches as safe.
        if (branches.length > 0) {
          const allSafe = await Promise.all(
            branches.map(async (branch) => {
              const isMerged = branch.prState === 'MERGED';
              if (!isMerged) return false;
              if (branch.branchType === 'remote') return true;

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
    workspaceLifecycle.enqueueInitialSetup(project.id, branches);
    replaceProjectRepos(project.id, repos);
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

    // Check if all branches have merged PRs and no unpushed changes.
    // Skip the expensive hasUnpushedCommits check for remote branches —
    // their commits live on the workspace and the SSH round-trip blocks the UI.
    const allSafe = await Promise.all(
      branches.map(async (branch) => {
        const isMerged = branch.prState === 'MERGED';
        if (!isMerged) return false;
        if (branch.branchType === 'remote') return true;

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
    const branchesToClear = branchesByProject.get(id) || [];
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

  async function handleRepoSelected(projectId: string, selection: RepoPickerSelection) {
    try {
      await commands.addProjectRepo(
        projectId,
        selection.nameWithOwner,
        selection.branchName,
        selection.subpath,
        undefined,
        selection.prNumber
      );
      const [projectsList, branches, repos] = await Promise.all([
        commands.listProjects(),
        commands.listBranchesForProject(projectId),
        commands.listProjectRepos(projectId),
      ]);
      projects = projectsList;
      branchesByProject = new Map(branchesByProject).set(projectId, branches);
      workspaceLifecycle.enqueueInitialSetup(projectId, branches);
      replaceProjectRepos(projectId, repos);
    } catch (e) {
      console.error('Failed to add repo:', e);
      const message = e instanceof Error ? e.message : String(e);
      alerts.show({
        tone: 'error',
        title: 'Unable to add repository',
        message,
        durationMs: 0,
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

  function handleWorkspaceStatusChange(
    projectId: string,
    branchId: string,
    workspaceStatus: WorkspaceStatus,
    workstationId?: number | null
  ) {
    workspaceLifecycle.handleWorkspaceStatusChange(
      projectId,
      branchId,
      workspaceStatus,
      workstationId
    );
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
            {reposById}
            canAddRepo={canAddRepo(project)}
            addRepoHint={project.location === 'remote' ? addRepoHint(project) : null}
            deleting={deletingProjectNames.has(project.id)}
            safeToDelete={safeToDeleteProjects.has(project.id)}
            {deletingBranches}
            {worktreeErrors}
            {workspaceErrors}
            detecting={detectingProjectIds.has(project.id)}
            onDeleteProject={() => handleDeleteProjectRequest(project)}
            onDeleteBranch={(branchId) => handleDeleteBranchRequest(branchId, project)}
            onRenameBranch={(branchId, branchName) =>
              handleRenameBranch(branchId, project.id, branchName)}
            onWorkspaceStatusChange={(branchId, workspaceStatus, workstationId) =>
              handleWorkspaceStatusChange(project.id, branchId, workspaceStatus, workstationId)}
            excludeRepos={new Set(
              [...reposById.values()]
                .filter((r) => r.projectId === project.id)
                .map((r) => r.githubRepo)
            )}
            onRepoSelected={(selection) => handleRepoSelected(project.id, selection)}
            onRetryWorktree={(branchId) => setupBranchWorktree(branchId, project.id)}
            onDismissReason={(projectRepoId) => {
              const repo = reposById.get(projectRepoId);
              if (repo) {
                reposById = new Map(reposById).set(projectRepoId, { ...repo, reason: null });
              }
            }}
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
    max-width: 900px;
    margin: 0 auto;
    padding: 16px 24px 24px;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: 32px;
  }
</style>
