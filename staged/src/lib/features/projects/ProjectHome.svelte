<!--
  ProjectHome.svelte - Project workspace page

  In app navigation this is the "project page". It can render a single selected
  project (detail view) or multiple projects when no filter is provided.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { ArrowLeft } from 'lucide-svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import type { Project, Branch, StoreIncompatibility, WorkspaceStatus } from '../../types';
  import * as commands from '../../commands';
  import { listenToRepoActionsDetection, runPrerunActions } from '../actions/actions';
  import { projectDisplayName } from '../../shared/utils';
  import { goHome, selectProject } from '../../navigation.svelte';
  import ProjectSection from './ProjectSection.svelte';
  import NewProjectModal from './NewProjectModal.svelte';
  import GitHubRepoPickerModal from './GitHubRepoPickerModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import StagedIcon from '../../shared/StagedIcon.svelte';
  import { alerts } from '../../shared/alerts.svelte';

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

  // Action detection state
  let detectingProjectIds = $state<Set<string>>(new Set());

  onMount(() => {
    checkStoreAndLoad();

    const onNewProject = () => handleNewProject();
    window.addEventListener('staged:new-project', onNewProject);

    let unlistenDetection: (() => void) | null = null;
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

    return () => {
      window.removeEventListener('staged:new-project', onNewProject);
      unlistenDetection?.();
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
    loading = true;
    error = null;
    try {
      const projectList = await commands.listProjects();
      projects = projectList;

      // Load branches for each project
      const branchMap = new Map<string, Branch[]>();
      const repoLabelMap = new Map<string, Map<string, string>>();
      await Promise.all(
        projectList.map(async (project) => {
          const [branches, repos] = await Promise.all([
            commands.listBranchesForProject(project.id),
            commands.listProjectRepos(project.id),
          ]);
          branchMap.set(project.id, branches);
          repoLabelMap.set(
            project.id,
            new Map(repos.map((repo) => [repo.id, repo.githubRepo] as const))
          );
        })
      );
      branchesByProject = branchMap;
      repoLabelsByProject = repoLabelMap;
      kickOffPendingBranchSetup(branchMap);

      const contexts = await commands.listActionContexts();
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
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  let visibleProjects = $derived(
    selectedProjectId ? projects.filter((project) => project.id === selectedProjectId) : projects
  );
  let hasContent = $derived(visibleProjects.length > 0);
  let selectedProject = $derived(
    selectedProjectId ? projects.find((project) => project.id === selectedProjectId) || null : null
  );

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

  function handleDeleteProjectRequest(project: Project) {
    projectToDelete = project;
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
      if (branch.branchType === 'local' && !branch.worktreePath) {
        setupBranchWorktree(branch.id, projectId).catch(() => {});
      } else if (branch.branchType === 'remote' && branch.workspaceStatus === 'starting') {
        commands.startWorkspace(branch.id).catch((e) => {
          console.error('[ProjectHome] Failed to start workspace:', e);
        });
      }
    }
  }

  function kickOffPendingBranchSetup(branchMap: Map<string, Branch[]>) {
    for (const [projectId, branches] of branchMap.entries()) {
      if (deletingProjectNames.has(projectId)) continue;
      for (const branch of branches) {
        if (pendingSetupBranches.has(branch.id)) continue;
        if (
          branch.branchType === 'local' &&
          !branch.worktreePath &&
          !worktreeErrors.has(branch.id)
        ) {
          setupBranchWorktree(branch.id, projectId).catch(() => {});
          continue;
        }
        if (branch.branchType === 'remote' && branch.workspaceStatus === 'starting') {
          pendingSetupBranches = new Set([...pendingSetupBranches, branch.id]);
          commands
            .startWorkspace(branch.id)
            .catch((e) => {
              console.error('[ProjectHome] Failed to start workspace:', e);
            })
            .finally(() => {
              const next = new Set(pendingSetupBranches);
              next.delete(branch.id);
              pendingSetupBranches = next;
            });
        }
      }
    }
  }

  $effect(() => {
    // Ensure pending setup starts even when we navigated to a project page
    // after creation and only loaded persisted branch records.
    if (!loading) {
      kickOffPendingBranchSetup(branchesByProject);
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
  <div class="content">
    {#if loading}
      <div class="loading-state">
        <p>Loading...</p>
      </div>
    {:else if storeIncompat && storeIncompat.kind === 'needs_reset'}
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
    {:else if !hasContent}
      <div class="empty-state">
        <div class="welcome-header">
          <StagedIcon size={28} />
          <h2>No projects yet</h2>
        </div>
        <p class="welcome-subtitle">
          Create a project to get started —
          <button class="kbd-btn" onclick={handleNewProject} title="New project">+</button>
          <span class="shortcut-hint">(⌘N)</span>
        </p>
      </div>
    {:else}
      {#if selectedProject}
        <div class="project-toolbar">
          <button class="back-button" onclick={goHome} title="Back to projects list">
            <ArrowLeft size={14} />
            Projects
          </button>
          <div class="project-title">{projectDisplayName(selectedProject)}</div>
        </div>
      {/if}
      <div class="projects-list">
        {#each visibleProjects as project (project.id)}
          <ProjectSection
            {project}
            branches={branchesByProject.get(project.id) || []}
            repoLabelsById={repoLabelsByProject.get(project.id) || new Map()}
            canAddRepo={canAddRepo(project)}
            addRepoHint={project.location === 'remote' ? addRepoHint(project) : null}
            deleting={deletingProjectNames.has(project.id)}
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

<!-- New project modal -->
{#if showNewProjectModal}
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
    message={`Remove "${projectDisplayName(projectToDelete)}" from Staged? This won't delete the repository.`}
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
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    background-color: var(--bg-chrome);
  }

  .content {
    flex: 1;
    overflow: auto;
    padding: 12px 24px 24px;
    display: flex;
    flex-direction: column;
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

  .loading-state,
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

  /* Empty state */
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    flex: 1;
    gap: 20px;
  }

  .welcome-header {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .welcome-header h2 {
    font-size: var(--size-xl);
    font-weight: 500;
    color: var(--text-primary);
    margin: 0;
  }

  .welcome-subtitle {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .welcome-subtitle .kbd-btn {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    font-size: var(--size-xs);
    padding: 1px 5px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--ui-accent);
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease;
  }

  .welcome-subtitle .kbd-btn:hover {
    background-color: var(--bg-hover);
    border-color: var(--ui-accent);
  }

  .welcome-subtitle .shortcut-hint {
    color: var(--text-faint);
    font-size: var(--size-xs);
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

  .project-toolbar {
    width: 100%;
    max-width: 800px;
    margin: 0 auto 16px;
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .back-button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: transparent;
    color: var(--text-muted);
    padding: 6px 10px;
    font-size: var(--size-xs);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .back-button:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
    background-color: var(--bg-hover);
  }

  .project-title {
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 600;
  }
</style>
