<!--
  ProjectSection.svelte - A project header + list of branch cards

  Shows the project name, repo controls, and all branch cards for this project.
-->
<script lang="ts">
  import { ChevronLeft, Trash2, Plus } from 'lucide-svelte';
  import type { Project, Branch, WorkspaceStatus } from '../../types';
  import { projectDisplayName } from '../../shared/utils';
  import { goHome } from '../../navigation.svelte';
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
      <button class="back-button" onclick={goHome} title="Back to projects">
        <ChevronLeft size={16} />
      </button>
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
      <div class="header-actions">
        <button
          class="header-action-button"
          onclick={() => onAddRepo?.()}
          disabled={addRepoDisabled}
          title={addRepoTitle}
        >
          <span class="action-icon"><Plus size={12} /></span>
          Add Repo
        </button>
        <button
          class="header-action-button danger"
          class:safe-delete={safeToDelete}
          onclick={() => onDeleteProject?.()}
          title="Remove project"
        >
          <span class="trash-icon"><Trash2 size={14} /></span>
          Remove Project
        </button>
      </div>
    {/if}
  </div>
  <div class="branches-list" class:deleting>
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

  .back-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background-color: transparent;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
    flex-shrink: 0;
  }

  .back-button:hover {
    color: var(--text-primary);
    background-color: var(--ui-selection);
  }

  .project-name {
    font-size: var(--size-xl);
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: -0.01em;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .header-action-button {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background-color: transparent;
    border: none;
    border-radius: 8px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    white-space: nowrap;
  }

  .header-action-button:hover {
    color: var(--text-primary);
    background-color: var(--ui-selection);
  }

  .header-action-button:hover .action-icon {
    background-color: var(--border-emphasis);
  }

  .header-action-button:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }

  .header-action-button:disabled:hover {
    color: var(--text-muted);
    background-color: transparent;
  }

  .header-action-button:disabled:hover .action-icon {
    background-color: var(--border-muted);
  }

  .action-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background-color: var(--border-muted);
    flex-shrink: 0;
  }

  .trash-icon {
    display: flex;
    align-items: center;
    color: var(--text-muted);
    transition: color 0.15s ease;
  }

  .header-action-button.danger:hover {
    color: var(--ui-danger);
  }

  .header-action-button.danger:hover .trash-icon {
    color: var(--ui-danger);
  }

  .header-action-button.safe-delete {
    color: var(--ui-danger);
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
</style>
