<!--
  ProjectSection.svelte - A project header + list of branch cards

  Shows the project name, repo controls, and all branch cards for this project.
-->
<script lang="ts">
  import { Folder, Trash2, Plus } from 'lucide-svelte';
  import type { Project, Branch, WorkspaceStatus } from '../../types';
  import { projectDisplayName } from '../../shared/utils';
  import BranchCard from '../branches/BranchCard.svelte';
  import RemoteBranchCard from '../branches/RemoteBranchCard.svelte';
  import Spinner from '../../shared/Spinner.svelte';

  interface Props {
    project: Project;
    branches: Branch[];
    repoLabelsById?: Map<string, string>;
    canAddRepo?: boolean;
    addRepoHint?: string | null;
    deleting?: boolean;
    safeToDelete?: boolean;
    deletingBranches?: Set<string>;
    worktreeErrors?: Map<string, string>;
    detecting?: boolean;
    onDeleteProject?: () => void;
    onDeleteBranch?: (branchId: string) => void;
    onRenameBranch?: (branchId: string, branchName: string) => void;
    onWorkspaceStatusChange?: (branchId: string, status: WorkspaceStatus) => void;
    onAddRepo?: () => void;
    onRetryWorktree?: (branchId: string) => void;
  }

  let {
    project,
    branches,
    repoLabelsById = new Map(),
    canAddRepo = true,
    addRepoHint = null,
    deleting = false,
    safeToDelete = false,
    deletingBranches = new Set(),
    worktreeErrors = new Map(),
    detecting = false,
    onDeleteProject,
    onDeleteBranch,
    onRenameBranch,
    onWorkspaceStatusChange,
    onAddRepo,
    onRetryWorktree,
  }: Props = $props();

  /** Branches sorted by most recently created first. */
  let sortedBranches = $derived([...branches].sort((a, b) => b.createdAt - a.createdAt));
  let addRepoDisabled = $derived(deleting || !canAddRepo);
  let addRepoTitle = $derived(
    deleting
      ? 'Project deletion in progress'
      : !canAddRepo && addRepoHint
        ? addRepoHint
        : 'Add repository to project'
  );

  function repoLabelForBranch(branch: Branch): string | null {
    if (!branch.projectRepoId) return project.githubRepo;
    return repoLabelsById.get(branch.projectRepoId) ?? project.githubRepo;
  }
</script>

<div class="project-section">
  <div class="project-header" class:deleting>
    <div class="project-info">
      <span class="folder-icon"><Folder size={14} /></span>
      <span class="project-name">{projectDisplayName(project)}</span>
      {#if deleting}
        <div class="deleting-status" role="status" aria-live="polite">
          <Spinner size={12} />
          <span>Deleting…</span>
        </div>
      {/if}
      {#if detecting}
        <div class="detecting-status">
          <Spinner size={12} />
          <span>Detecting actions</span>
        </div>
      {/if}
    </div>
    {#if !deleting}
      <button
        class="remove-button"
        class:safe-delete={safeToDelete}
        onclick={() => onDeleteProject?.()}
        title="Remove project"
      >
        <Trash2 size={14} />
        Remove Project
      </button>
    {/if}
  </div>
  <div class="branches-list" class:deleting>
    <button
      class="manage-repos-button"
      onclick={() => onAddRepo?.()}
      disabled={addRepoDisabled}
      title={addRepoTitle}
    >
      <Plus size={16} />
      Add Repo
    </button>
    {#if !deleting && !canAddRepo && addRepoHint}
      <div class="repo-hint">{addRepoHint}</div>
    {/if}
    {#each sortedBranches as branch (branch.id)}
      {#if branch.branchType === 'remote'}
        <RemoteBranchCard
          {branch}
          repoLabel={repoLabelForBranch(branch)}
          deleting={deletingBranches.has(branch.id)}
          onDelete={() => onDeleteBranch?.(branch.id)}
          onRename={(branchName) => onRenameBranch?.(branch.id, branchName)}
          onWorkspaceStatusChange={(status) => onWorkspaceStatusChange?.(branch.id, status)}
        />
      {:else}
        <BranchCard
          {branch}
          repoLabel={repoLabelForBranch(branch)}
          deleting={deletingBranches.has(branch.id)}
          worktreeError={worktreeErrors.get(branch.id)}
          onDelete={() => onDeleteBranch?.(branch.id)}
          onRename={(branchName) => onRenameBranch?.(branch.id, branchName)}
          onRetryWorktree={() => onRetryWorktree?.(branch.id)}
        />
      {/if}
    {/each}
  </div>
</div>

<style>
  .project-section {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .project-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 4px;
  }

  .project-info {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .folder-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-faint);
  }

  .project-name {
    font-size: var(--size-xl);
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: -0.01em;
  }

  .remove-button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    white-space: nowrap;
  }

  .remove-button:hover {
    background-color: var(--bg-danger-hover);
    border-color: var(--ui-danger);
    color: var(--text-danger);
  }

  .remove-button.safe-delete {
    border-color: var(--ui-danger);
    color: var(--ui-danger);
  }

  .remove-button.safe-delete:hover {
    background-color: var(--ui-danger);
    border-color: var(--ui-danger);
    color: white;
  }

  .detecting-status {
    height: 22px;
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 8px;
    padding: 0 10px;
    border-radius: 999px;
    background-color: var(--bg-primary);
    color: var(--text-primary);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
    line-height: 1;
    border: 1px solid var(--border-muted);
  }

  .deleting-status {
    height: 22px;
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 8px;
    padding: 0 10px;
    border-radius: 999px;
    background-color: var(--bg-primary);
    color: var(--text-primary);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
    line-height: 1;
    border: 1px solid var(--border-muted);
  }

  .branches-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .branches-list.deleting {
    opacity: 0.65;
    pointer-events: none;
  }

  .manage-repos-button {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 20px;
    background-color: transparent;
    border: 1px dashed var(--border-muted);
    border-radius: 8px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .manage-repos-button:hover {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .manage-repos-button:disabled {
    cursor: not-allowed;
    opacity: 0.6;
    border-color: var(--border-muted);
    color: var(--text-muted);
    background-color: transparent;
  }

  .repo-hint {
    margin-top: -4px;
    padding: 0 4px;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }
</style>
