<!--
  ProjectSettingsModal.svelte - Manage project repositories
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X, Plus, Trash2, FolderGit2, Pencil } from 'lucide-svelte';
  import type { Branch, Project, ProjectRepo } from '../../types';
  import * as commands from '../../commands';
  import GitHubRepoPickerModal from './GitHubRepoPickerModal.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import { alerts } from '../../shared/alerts.svelte';

  interface Props {
    project: Project;
    detecting?: boolean;
    onClose: () => void;
  }

  let { project, onClose }: Props = $props();

  let projectRepos = $state<ProjectRepo[]>([]);
  let savingBranchIds = $state<Set<string>>(new Set());
  let renamingRepo = $state<ProjectRepo | null>(null);
  let repoToRemove = $state<ProjectRepo | null>(null);
  let renameDraft = $state('');
  let loadingRepos = $state(false);
  let showRepoPicker = $state(false);
  let branches = $state<Branch[]>([]);

  function errorMessageFrom(e: unknown): string {
    if (e instanceof Error) return e.message;
    return String(e);
  }

  onMount(() => {
    loadRepos();
  });

  async function loadRepos() {
    loadingRepos = true;
    try {
      const [repos, branchList] = await Promise.all([
        commands.listProjectRepos(project.id),
        commands.listBranchesForProject(project.id),
      ]);
      projectRepos = repos;
      branches = branchList;
    } catch (e) {
      console.error('Failed to load project repos:', e);
      alerts.show({
        tone: 'error',
        title: 'Unable to load repositories',
        message: errorMessageFrom(e),
        durationMs: 0,
      });
    } finally {
      loadingRepos = false;
    }
  }

  async function addRepo(githubRepo: string, subpath?: string) {
    if (!canAddRepo()) {
      alerts.show({
        tone: 'warning',
        title: 'Unable to add repository',
        message: addRepoHint(),
      });
      return;
    }
    try {
      await commands.addProjectRepo(
        project.id,
        githubRepo,
        undefined,
        subpath,
        projectRepos.length === 0
      );
      await loadRepos();
      showRepoPicker = false;
    } catch (e) {
      console.error('Failed to add repo:', e);
      alerts.show({
        tone: 'error',
        title: 'Unable to add repository',
        message: errorMessageFrom(e),
        durationMs: 0,
      });
    }
  }

  async function removeRepo(repoId: string) {
    try {
      await commands.removeProjectRepo(project.id, repoId);
      await loadRepos();
    } catch (e) {
      console.error('Failed to remove repo:', e);
      alerts.show({
        tone: 'error',
        title: 'Unable to remove repository',
        message: errorMessageFrom(e),
        durationMs: 0,
      });
    }
  }

  function requestRemoveRepo(repo: ProjectRepo) {
    repoToRemove = repo;
  }

  async function confirmRemoveRepo() {
    if (!repoToRemove) return;
    const repoId = repoToRemove.id;
    repoToRemove = null;
    await removeRepo(repoId);
  }

  function startRename(repo: ProjectRepo) {
    renamingRepo = repo;
    renameDraft = repo.branchName;
  }

  function cancelRename() {
    renamingRepo = null;
    renameDraft = '';
  }

  async function saveRepoBranch() {
    if (!renamingRepo) return;
    const repoId = renamingRepo.id;
    const branchName = renameDraft.trim();
    if (!branchName) return;
    if (savingBranchIds.has(repoId)) return;
    try {
      savingBranchIds = new Set([...savingBranchIds, repoId]);
      await commands.updateProjectRepoBranchName(project.id, repoId, branchName);
      projectRepos = projectRepos.map((repo) =>
        repo.id === repoId ? { ...repo, branchName } : repo
      );
    } catch (e) {
      console.error('Failed to update repo branch name:', e);
      alerts.show({
        tone: 'error',
        title: 'Unable to rename branch',
        message: errorMessageFrom(e),
        durationMs: 0,
      });
      await loadRepos();
    } finally {
      const next = new Set(savingBranchIds);
      next.delete(repoId);
      savingBranchIds = next;
      cancelRename();
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      onClose();
      event.preventDefault();
    }
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      onClose();
    }
  }

  function canAddRepo(): boolean {
    if (project.location !== 'remote') return true;
    return branches.some((b) => b.branchType === 'remote' && b.workspaceStatus === 'running');
  }

  function addRepoHint(): string {
    if (project.location !== 'remote') return '';
    if (branches.some((b) => b.branchType === 'remote' && b.workspaceStatus === 'starting')) {
      return 'Workspace is provisioning. Wait until it is running, then add another repo.';
    }
    return 'Workspace must be running before adding another repo.';
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={(e) => e.key === 'Escape' && onClose()}
>
  <div class="modal">
    <header class="modal-header">
      <h2>
        <FolderGit2 size={16} />
        Manage Repositories
      </h2>
      <button class="close-btn" onclick={onClose}>
        <X size={16} />
      </button>
    </header>

    <div class="modal-body">
      <div class="repos-header">
        <div class="section-title">Repositories</div>
        <button
          class="secondary-btn"
          onclick={() => (showRepoPicker = true)}
          disabled={!canAddRepo()}
          title={!canAddRepo() ? addRepoHint() : 'Add repository'}
        >
          <Plus size={14} />
          Add Repo
        </button>
      </div>
      <div class="repo-hint">
        Each repo has a default branch name used when Staged creates its branch/worktree.
      </div>
      {#if !canAddRepo()}
        <div class="repo-hint">{addRepoHint()}</div>
      {/if}

      {#if loadingRepos}
        <div class="empty-hint">Loading repositories...</div>
      {:else if projectRepos.length === 0}
        <div class="empty-hint">No repositories attached.</div>
      {:else}
        <div class="repos-list">
          {#each projectRepos as repo (repo.id)}
            <div class="repo-item">
              <div class="repo-main">
                <div class="repo-title">
                  <code>{repo.githubRepo}</code>
                </div>
                <div class="repo-branch-label">Branch: <code>{repo.branchName}</code></div>
              </div>
              <div class="action-controls">
                <button class="icon-btn" onclick={() => startRename(repo)} title="Rename branch">
                  <Pencil size={14} />
                </button>
                <button
                  class="icon-btn danger"
                  onclick={() => requestRemoveRepo(repo)}
                  title="Remove"
                >
                  <Trash2 size={14} />
                </button>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</div>

{#if renamingRepo}
  <div
    class="rename-backdrop"
    role="presentation"
    tabindex="-1"
    onclick={(event) => {
      if (event.target === event.currentTarget) cancelRename();
    }}
    onkeydown={(event) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        cancelRename();
      }
    }}
  >
    <div class="rename-modal" role="dialog" aria-modal="true">
      <div class="rename-title">Rename Branch</div>
      <div class="rename-repo">{renamingRepo.githubRepo}</div>
      <input
        value={renameDraft}
        oninput={(event) => {
          const target = event.currentTarget as HTMLInputElement;
          renameDraft = target.value;
        }}
        onkeydown={(event) => {
          if (event.key === 'Enter') {
            event.preventDefault();
            saveRepoBranch();
          } else if (event.key === 'Escape') {
            event.preventDefault();
            cancelRename();
          }
        }}
        placeholder="Branch name"
      />
      <div class="rename-actions">
        <button class="secondary-btn" onclick={cancelRename}>Cancel</button>
        <button
          class="primary-btn"
          onclick={saveRepoBranch}
          disabled={!renameDraft.trim() || savingBranchIds.has(renamingRepo.id)}
        >
          Save
        </button>
      </div>
    </div>
  </div>
{/if}

{#if showRepoPicker}
  <GitHubRepoPickerModal onSelect={addRepo} onClose={() => (showRepoPicker = false)} />
{/if}

{#if repoToRemove}
  <ConfirmDialog
    title="Remove Repository"
    message={`Remove "${repoToRemove.githubRepo}" from this project? Existing branch/worktree history in Staged for that repo may be affected.`}
    confirmLabel="Remove"
    danger={true}
    onConfirm={confirmRemoveRepo}
    onCancel={() => (repoToRemove = null)}
  />
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    background: var(--bg-chrome);
    border-radius: 12px;
    width: min(640px, 90vw);
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    box-shadow: var(--shadow-elevated);
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .modal-header h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .close-btn {
    background: none;
    border: none;
    color: var(--text-secondary);
    cursor: pointer;
    padding: 4px;
    display: flex;
    align-items: center;
    border-radius: 4px;
  }

  .close-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .modal-body {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px;
    overflow-y: auto;
  }

  .repos-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .section-title {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  .repos-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .repo-item {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    padding: 12px;
    border-radius: 8px;
    border: 1px solid var(--border-subtle);
    background: var(--bg-primary);
  }

  .repo-main {
    min-width: 0;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .repo-title {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .repo-title code {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-branch-label {
    margin-top: 2px;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .secondary-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px;
    border-radius: 6px;
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    background: var(--bg-primary);
    color: var(--text-primary);
    border: 1px solid var(--border-muted);
  }

  .secondary-btn:hover {
    background: var(--bg-hover);
    border-color: var(--border-emphasis);
  }

  .secondary-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .primary-btn {
    background: var(--ui-accent);
    color: var(--bg-primary);
    border: none;
    border-radius: 6px;
    padding: 8px 14px;
    font-size: var(--size-xs);
    font-weight: 600;
    cursor: pointer;
  }

  .primary-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .action-controls {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .icon-btn {
    width: 30px;
    height: 30px;
    border-radius: 6px;
    border: 1px solid var(--border-muted);
    background: var(--bg-primary);
    color: var(--text-secondary);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
  }

  .icon-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
    background: var(--bg-hover);
  }

  .icon-btn.danger:hover {
    color: var(--ui-danger);
    border-color: var(--ui-danger);
  }

  .empty-hint {
    font-size: 12px;
    color: var(--text-tertiary);
    padding: 10px 2px;
  }

  .repo-hint {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .rename-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1001;
  }

  .rename-modal {
    width: min(420px, 90vw);
    background: var(--bg-chrome);
    border: 1px solid var(--border-subtle);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .rename-title {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  .rename-repo {
    font-size: var(--size-xs);
    color: var(--text-muted);
    font-family: 'SF Mono', 'Menlo', monospace;
  }

  .rename-modal input {
    border: 1px solid var(--border-muted);
    background: var(--bg-deepest);
    color: var(--text-primary);
    border-radius: 8px;
    padding: 8px 10px;
    font-size: var(--size-sm);
  }

  .rename-modal input:focus {
    outline: none;
    border-color: var(--ui-accent);
  }

  .rename-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
