<!--
  NewProjectModal.svelte - Create a named project

  A project can be created with or without a repository.
-->
<script lang="ts">
  import { X, GitBranch } from 'lucide-svelte';
  import type { Project } from '../../types';
  import * as commands from '../../commands';
  import GitHubRepoPickerModal from './GitHubRepoPickerModal.svelte';
  import { detectProjectActions } from '../actions/actions';

  interface Props {
    onCreated: (project: Project) => void;
    onDetecting: (projectId: string, detecting: boolean) => void;
    onClose: () => void;
  }

  let { onCreated, onDetecting, onClose }: Props = $props();

  let name = $state('');
  let selectedRepo = $state<string | null>(null);
  let subpath = $state('');
  let saving = $state(false);
  let error = $state<string | null>(null);
  let showRepoPicker = $state(false);

  async function handleCreate() {
    if (!name.trim() || saving) return;

    saving = true;
    error = null;

    try {
      const normalizedSubpath = selectedRepo
        ? (subpath.trim().replace(/^\/+|\/+$/g, '') || undefined)
        : undefined;

      const project = await commands.createProject(
        name.trim(),
        selectedRepo ?? undefined,
        normalizedSubpath
      );

      if (selectedRepo) {
        detectAndSaveActions(project.id).catch(() => {});
        onDetecting(project.id, true);
      }

      onCreated(project);
    } catch (e) {
      if (typeof e === 'string') {
        error = e;
      } else if (e instanceof Error) {
        error = e.message;
      } else {
        error = String(e);
      }
      saving = false;
    }
  }

  async function detectAndSaveActions(projectId: string) {
    try {
      const suggested = await detectProjectActions(projectId);

      for (let i = 0; i < suggested.length; i++) {
        const action = suggested[i];
        await commands.createProjectAction(
          projectId,
          action.name,
          action.command,
          action.actionType,
          i,
          action.autoCommit
        );
      }
    } finally {
      onDetecting(projectId, false);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      handleCreate();
    }
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      onClose();
    }
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
    <div class="modal-header">
      <h2>New Project</h2>
      <button class="close-button" onclick={onClose}>
        <X size={18} />
      </button>
    </div>

    <div class="modal-body">
      <div class="form-group">
        <label for="project-name">Name</label>
        <input
          bind:value={name}
          id="project-name"
          type="text"
          placeholder="e.g., Billing Platform"
          disabled={saving}
          autocomplete="off"
        />
      </div>

      <div class="form-group">
        <label for="project-repo-select">Repository <span class="optional-label">(optional)</span></label>
        {#if selectedRepo}
          <div class="repo-info">
            <GitBranch size={14} class="repo-info-icon" />
            <div class="repo-details">
              <span class="repo-name">{selectedRepo}</span>
            </div>
            <button class="change-button" onclick={() => (showRepoPicker = true)}>Change</button>
            <button class="change-button" onclick={() => (selectedRepo = null)}>Clear</button>
          </div>
        {:else}
          <button
            id="project-repo-select"
            class="select-repo-button"
            onclick={() => (showRepoPicker = true)}
          >
            Select repository
          </button>
        {/if}
      </div>

      {#if selectedRepo}
        <div class="form-group">
          <label for="project-subpath">Subpath <span class="optional-label">(optional)</span></label>
          <input
            bind:value={subpath}
            id="project-subpath"
            type="text"
            placeholder="e.g., packages/frontend"
            disabled={saving}
            autocomplete="off"
            autocorrect="off"
            autocapitalize="off"
            spellcheck="false"
          />
        </div>
      {/if}

      {#if error}
        <div class="error-message">{error}</div>
      {/if}

      <div class="actions">
        <button class="cancel-button" onclick={onClose} disabled={saving}>Cancel</button>
        <button class="create-button" onclick={handleCreate} disabled={saving || !name.trim()}>
          {saving ? 'Creating...' : 'Create Project'}
        </button>
      </div>
    </div>
  </div>
</div>

{#if showRepoPicker}
  <GitHubRepoPickerModal
    onSelect={(nameWithOwner) => {
      selectedRepo = nameWithOwner;
      showRepoPicker = false;
    }}
    onClose={() => (showRepoPicker = false)}
  />
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background-color: var(--shadow-overlay);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 12vh;
    z-index: 1000;
  }

  .modal {
    width: 460px;
    max-width: 90vw;
    background-color: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    display: flex;
    flex-direction: column;
  }

  .modal-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 16px;
    border-bottom: 1px solid var(--border-subtle);
    border-radius: 12px 12px 0 0;
  }

  .modal-header h2 {
    flex: 1;
    margin: 0;
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-primary);
  }

  .close-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
  }

  .close-button:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .modal-body {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  label {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .optional-label {
    color: var(--text-faint);
  }

  input {
    border: 1px solid var(--border-muted);
    background: var(--bg-deepest);
    color: var(--text-primary);
    border-radius: 8px;
    padding: 9px 10px;
    font-size: var(--size-sm);
    outline: none;
  }

  input:focus {
    border-color: var(--ui-accent);
  }

  .repo-info {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    background-color: var(--bg-hover);
    border-radius: 8px;
  }

  .repo-details {
    min-width: 0;
    flex: 1;
  }

  .repo-name {
    font-size: var(--size-sm);
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .change-button,
  .select-repo-button {
    border: 1px solid var(--border-muted);
    border-radius: 7px;
    background: transparent;
    color: var(--text-muted);
    padding: 6px 10px;
    cursor: pointer;
  }

  .change-button:hover,
  .select-repo-button:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
    background: var(--bg-hover);
  }

  .error-message {
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .cancel-button,
  .create-button {
    border-radius: 8px;
    padding: 8px 12px;
    border: 1px solid var(--border-muted);
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
  }

  .create-button {
    background: var(--ui-accent);
    border-color: var(--ui-accent);
    color: var(--bg-deepest);
    font-weight: 600;
  }

  .create-button:disabled,
  .cancel-button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
