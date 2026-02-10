<!--
  ProjectSection.svelte - A project header + list of branch cards

  Shows the project name, a delete button, and all branches for this project.
  Includes a "New Branch" dashed button at the bottom.
-->
<script lang="ts">
  import { Folder, Trash2, Plus } from 'lucide-svelte';
  import type { Project, Branch } from './types';
  import { projectDisplayName } from './utils';
  import BranchCard from './BranchCard.svelte';
  import DropdownMenu, { type MenuItem } from './DropdownMenu.svelte';

  interface Props {
    project: Project;
    branches: Branch[];
    onDeleteProject?: () => void;
    onDeleteBranch?: (branchId: string) => void;
    onNewBranch?: () => void;
  }

  let { project, branches, onDeleteProject, onDeleteBranch, onNewBranch }: Props = $props();

  const projectMenuItems: MenuItem[] = [
    { label: 'Remove Project', icon: Trash2, danger: true, action: () => onDeleteProject?.() },
  ];
</script>

<div class="project-section">
  <div class="project-header">
    <div class="project-info">
      <div class="project-icon-slot">
        <span class="folder-icon"><Folder size={14} /></span>
        <span class="menu-icon"><DropdownMenu items={projectMenuItems} align="left" /></span>
      </div>
      <span class="project-name">{projectDisplayName(project)}</span>
    </div>
  </div>
  <div class="branches-list">
    {#each branches as branch (branch.id)}
      <BranchCard {branch} onDelete={() => onDeleteBranch?.(branch.id)} />
    {/each}
    <!-- New branch button -->
    <button class="new-branch-button" onclick={() => onNewBranch?.()}>
      <Plus size={16} />
      New Branch
    </button>
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
    padding: 0 4px;
  }

  .project-info {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .project-icon-slot {
    position: relative;
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .folder-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-faint);
    transition: opacity 0.15s ease;
  }

  .menu-icon {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    transition: opacity 0.15s ease;
  }

  .project-header:hover .folder-icon {
    opacity: 0;
  }

  .project-header:hover .menu-icon {
    opacity: 1;
  }

  .project-name {
    font-size: var(--size-lg);
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: -0.01em;
  }

  .branches-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .new-branch-button {
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

  .new-branch-button:hover {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
    background-color: var(--bg-hover);
  }
</style>
