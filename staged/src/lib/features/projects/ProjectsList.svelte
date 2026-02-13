<!--
  ProjectsList.svelte — Landing page showing all projects with metadata

  Fetches commit timelines for every tracked branch to derive:
  - Recent commits per project (top 5 across all branches)
  - Last activity timestamp (most recent commit)

  Empty state shows the welcome UI with StagedIcon and GitTreeAnimation.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import type { Project, Branch } from '../../types';
  import * as commands from '../../commands';
  import { projectDisplayName } from '../../shared/utils';
  import { selectProject } from '../../navigation.svelte';
  import {
    projectStore,
    checkStoreAndLoad,
    handleResetStore,
    addProject,
    removeProject,
  } from './projectStore.svelte';
  import ProjectCard from './ProjectCard.svelte';
  import NewProjectModal from './NewProjectModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import GitTreeAnimation from '../../shared/GitTreeAnimation.svelte';
  import StagedIcon from '../../shared/StagedIcon.svelte';

  // ── Modal state ──

  let showNewProjectModal = $state(false);
  let projectToDelete = $state<Project | null>(null);

  // ── Action detection state ──

  let detectingProjectIds = $state<Set<string>>(new Set());

  // ── Timeline data per project ──

  type ProjectCommitInfo = {
    shortSha: string;
    subject: string;
    timestamp: number;
  };

  let commitsByProject = $state(new Map<string, ProjectCommitInfo[]>());
  let timelineLoading = $state(false);
  let lastBranchKey = '';

  /** Strip XML-tagged context blocks from display text. */
  function stripXmlTags(text: string): string {
    return text.replace(/<(action|branch-history)>[\s\S]*?<\/\1>/g, '').trim();
  }

  // Re-fetch timelines when branches change
  $effect(() => {
    const allBranches: Branch[] = [];
    for (const project of projectStore.projects) {
      const branches = projectStore.branchesByProject.get(project.id) || [];
      allBranches.push(...branches);
    }
    const branchKey = allBranches
      .map((b) => b.id)
      .sort()
      .join(',');

    if (branchKey === lastBranchKey || projectStore.loading) return;
    lastBranchKey = branchKey;

    if (allBranches.length === 0) {
      commitsByProject = new Map();
      return;
    }

    fetchAllTimelines(allBranches);
  });

  async function fetchAllTimelines(allBranches: Branch[]) {
    timelineLoading = true;
    try {
      const result = new Map<string, ProjectCommitInfo[]>();

      await Promise.all(
        allBranches.map(async (branch) => {
          try {
            const tl = await commands.getBranchTimeline(branch.id);
            const existing = result.get(branch.projectId) || [];

            for (const commit of tl.commits) {
              if (!commit.sha) continue;
              existing.push({
                shortSha: commit.shortSha,
                subject: stripXmlTags(commit.subject),
                timestamp: commit.timestamp,
              });
            }

            result.set(branch.projectId, existing);
          } catch {
            // Skip branches that fail silently
          }
        })
      );

      // Sort each project's commits by timestamp descending
      for (const [pid, commits] of result) {
        commits.sort((a, b) => b.timestamp - a.timestamp);
        result.set(pid, commits);
      }

      commitsByProject = result;
    } finally {
      timelineLoading = false;
    }
  }

  // ── Relative time formatting ──

  function formatRelativeTime(timestamp: number): string {
    const date = new Date(timestamp * 1000);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    const diffHours = Math.floor(diffMins / 60);
    const diffDays = Math.floor(diffHours / 24);

    if (diffMins < 1) return 'just now';
    if (diffMins < 60) return `${diffMins}m`;
    if (diffHours < 24) return `${diffHours}h`;
    if (diffDays < 7) return `${diffDays}d`;
    return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  }

  // ── Derived data for each project card ──

  const MAX_RECENT_COMMITS = 5;

  function getRecentCommits(projectId: string) {
    const commits = commitsByProject.get(projectId) || [];
    return commits.slice(0, MAX_RECENT_COMMITS).map((c) => ({
      shortSha: c.shortSha,
      subject: c.subject,
      relativeTime: formatRelativeTime(c.timestamp),
    }));
  }

  function getLastActivity(projectId: string): string {
    const commits = commitsByProject.get(projectId) || [];
    if (commits.length > 0) {
      return formatRelativeTime(commits[0].timestamp);
    }
    // Fall back to branch updatedAt
    const branches = projectStore.branchesByProject.get(projectId) || [];
    if (branches.length > 0) {
      const latest = Math.max(...branches.map((b) => b.updatedAt));
      return formatRelativeTime(Math.floor(latest / 1000));
    }
    return '';
  }

  function getBranchCount(projectId: string): number {
    return (projectStore.branchesByProject.get(projectId) || []).length;
  }

  // ── Event handlers ──

  onMount(() => {
    checkStoreAndLoad();

    const onNewProject = () => handleNewProject();
    window.addEventListener('staged:new-project', onNewProject);
    return () => window.removeEventListener('staged:new-project', onNewProject);
  });

  function handleClose() {
    getCurrentWindow().close();
  }

  let hasContent = $derived(projectStore.projects.length > 0);

  function handleNewProject() {
    showNewProjectModal = true;
  }

  async function handleProjectCreated(project: Project) {
    await addProject(project);
    showNewProjectModal = false;
    selectProject(project.id);
  }

  function handleProjectDetecting(projectId: string, detecting: boolean) {
    if (detecting) {
      detectingProjectIds = new Set([...detectingProjectIds, projectId]);
    } else {
      const next = new Set(detectingProjectIds);
      next.delete(projectId);
      detectingProjectIds = next;
    }
  }

  function handleDeleteProjectRequest(project: Project) {
    projectToDelete = project;
  }

  async function confirmDeleteProject() {
    if (!projectToDelete) return;
    const id = projectToDelete.id;
    projectToDelete = null;
    try {
      await removeProject(id);
    } catch (e) {
      console.error('Failed to delete project:', e);
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

<div class="projects-list-view">
  <div class="content">
    {#if projectStore.loading}
      <div class="loading-state">
        <p>Loading...</p>
      </div>
    {:else if projectStore.storeIncompat && projectStore.storeIncompat.kind === 'needs_reset'}
      <div class="update-state">
        <div class="update-card">
          <div class="update-header">
            <h1 class="update-title">Update Required</h1>
            <span class="version-badge new">v{projectStore.storeIncompat.appVersion}</span>
          </div>
          <p>
            Staged beta updates can require backwards-incompatible changes. The info stored by
            Staged (session history, notes) will be cleared, but your
            <strong>git repos and branches are not affected</strong>.
          </p>
          <div class="update-footer">
            <p class="version-hint">
              Not ready? Install <code>v{projectStore.storeIncompat.dbAppVersion}</code> instead.
            </p>
            <div class="update-actions">
              <button class="close-button" onclick={handleClose}>Close</button>
              <button
                class="reset-button"
                onclick={handleResetStore}
                disabled={projectStore.resetting}
              >
                {projectStore.resetting ? 'Resetting…' : 'Reset & Update'}
              </button>
            </div>
          </div>
        </div>
      </div>
    {:else if projectStore.storeIncompat && projectStore.storeIncompat.kind === 'too_new'}
      <div class="update-state">
        <div class="update-card">
          <div class="update-header">
            <h1 class="update-title">Update Staged</h1>
            <span class="version-badge new">v{projectStore.storeIncompat.dbAppVersion}</span>
          </div>
          <p>
            This database was last used by a newer version of Staged. Please install
            <strong>v{projectStore.storeIncompat.dbAppVersion}</strong> or newer to continue.
          </p>
          <div class="update-footer">
            <div></div>
            <div class="update-actions">
              <button class="close-button" onclick={handleClose}>Close</button>
            </div>
          </div>
        </div>
      </div>
    {:else if projectStore.error}
      <div class="error-state">
        <p>{projectStore.error}</p>
      </div>
    {:else if !hasContent}
      <div class="empty-state">
        <div class="welcome-header">
          <StagedIcon size={28} />
          <h2>welcome to <span class="mono accent">staged</span></h2>
        </div>
        <p class="welcome-subtitle">
          Add one of your repos as a project to get started —
          <button class="kbd-btn" onclick={handleNewProject} title="New project">+</button>
          <span class="shortcut-hint">(⌘N)</span>
        </p>
        <GitTreeAnimation />
      </div>
    {:else}
      <div class="projects-grid">
        {#each projectStore.projects as project (project.id)}
          <ProjectCard
            {project}
            branchCount={getBranchCount(project.id)}
            lastActivity={getLastActivity(project.id)}
            recentCommits={getRecentCommits(project.id)}
          />
        {/each}
      </div>
    {/if}
  </div>
</div>

<!-- New project modal -->
{#if showNewProjectModal}
  <NewProjectModal
    onCreated={handleProjectCreated}
    onDetecting={handleProjectDetecting}
    onClose={() => (showNewProjectModal = false)}
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

<style>
  .projects-list-view {
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

  .welcome-header .mono {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    letter-spacing: -0.02em;
  }

  .welcome-header .accent {
    color: var(--ui-accent);
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

  /* Projects grid */
  .projects-grid {
    width: 100%;
    max-width: 800px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
</style>
