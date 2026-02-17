<!--
  NewProjectModal.svelte - Create a named project

  A project can be created with or without a repository.
-->
<script lang="ts">
  import { X, GitBranch, Plus } from 'lucide-svelte';
  import type { Project, RecentRepo } from '../../types';
  import * as commands from '../../commands';
  import GitHubRepoPickerModal from './GitHubRepoPickerModal.svelte';
  import { onMount } from 'svelte';

  interface Props {
    onCreated: (project: Project) => void;
    onClose: () => void;
  }

  let { onCreated, onClose }: Props = $props();

  let name = $state('');
  let location = $state<'local' | 'remote'>('local');
  let selectedRepo = $state<string | null>(null);
  let subpath = $state('');
  let saving = $state(false);
  let error = $state<string | null>(null);
  let showRepoPicker = $state(false);
  let recentRepos = $state<RecentRepo[]>([]);

  onMount(async () => {
    try {
      recentRepos = await commands.listRecentRepos(3);
    } catch (e) {
      // Fail silently - recent repos are optional
    }
  });

  async function handleCreate() {
    if (!name.trim() || saving) return;

    saving = true;
    error = null;

    try {
      const normalizedSubpath = selectedRepo
        ? subpath.trim().replace(/^\/+|\/+$/g, '') || undefined
        : undefined;

      const project = await commands.createProject(
        name.trim(),
        location,
        selectedRepo ?? undefined,
        normalizedSubpath
      );
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
        <div class="field-label">Location</div>
        <div class="type-toggle">
          <button
            class="toggle-option"
            class:active={location === 'local'}
            onclick={() => (location = 'local')}
            disabled={saving}
          >
            Local
          </button>
          <button
            class="toggle-option"
            class:active={location === 'remote'}
            onclick={() => (location = 'remote')}
            disabled={saving}
          >
            Remote
          </button>
        </div>
      </div>

      <div class="form-group">
        <label for="project-repo-select"
          >Repository <span class="optional-label">(optional)</span></label
        >
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
          {#if recentRepos.length > 0}
            <div class="recent-repos-section">
              {#each recentRepos as recent}
                <button
                  class="recent-repo-item"
                  onclick={() => {
                    selectedRepo = recent.githubRepo;
                    if (recent.subpath) {
                      subpath = recent.subpath;
                    }
                  }}
                >
                  <Plus size={14} class="recent-icon" />
                  <div class="recent-repo-info">
                    <span class="recent-repo-name">
                      {recent.githubRepo}{#if recent.subpath}<span class="recent-repo-subpath"
                          >/{recent.subpath}</span
                        >{/if}
                    </span>
                  </div>
                </button>
              {/each}
            </div>
          {/if}
        {/if}
      </div>

      {#if selectedRepo}
        <div class="form-group">
          <label for="project-subpath">Subpath <span class="optional-label">(optional)</span></label
          >
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
    onSelect={(nameWithOwner, selectedSubpath) => {
      selectedRepo = nameWithOwner;
      if (selectedSubpath) {
        subpath = selectedSubpath;
      }
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

  .field-label {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .optional-label {
    color: var(--text-faint);
  }

  .type-toggle {
    display: flex;
    gap: 6px;
  }

  .toggle-option {
    flex: 1;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: transparent;
    color: var(--text-muted);
    padding: 8px 10px;
    cursor: pointer;
  }

  .toggle-option.active {
    border-color: var(--ui-accent);
    color: var(--text-primary);
    background-color: var(--bg-hover);
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

  .recent-repos-section {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 8px;
  }

  .recent-repo-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    background: var(--bg-deepest);
    border: 1px solid var(--border-muted);
    border-left: 2px solid var(--ui-accent);
    border-radius: 7px;
    cursor: pointer;
    text-align: left;
  }

  .recent-repo-item:hover {
    background: var(--bg-hover);
    border-color: var(--border-emphasis);
  }

  :global(.recent-icon) {
    color: var(--ui-accent);
    flex-shrink: 0;
  }

  .recent-repo-info {
    flex: 1;
    min-width: 0;
  }

  .recent-repo-name {
    font-size: var(--size-sm);
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .recent-repo-subpath {
    font-size: var(--size-sm);
    color: var(--text-muted);
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
