<!--
  NewProjectForm.svelte - Reusable project creation form

  Contains the form fields, state, and logic for creating a new project.
  Used inside NewProjectModal (as a dialog) and SplashScreen (inline).
-->
<script lang="ts">
  import { GitBranch, Plus, Command } from 'lucide-svelte';
  import type { Project, RecentRepo } from '../../types';
  import * as commands from '../../commands';
  import GitHubRepoPickerModal from './GitHubRepoPickerModal.svelte';
  import { onMount } from 'svelte';

  interface Props {
    onCreated: (project: Project) => void;
    onCancel?: () => void;
  }

  let { onCreated, onCancel }: Props = $props();

  let name = $state('');
  let location = $state<'local' | 'remote'>('local');
  let selectedRepo = $state<string | null>(null);
  let subpath = $state('');
  let saving = $state(false);
  let error = $state<string | null>(null);
  let showRepoPicker = $state(false);
  let recentRepos = $state<RecentRepo[]>([]);
  let isMonorepo = $state(false);
  let checkingMonorepo = $state(false);

  function focus(node: HTMLElement) {
    node.focus();
  }

  onMount(async () => {
    try {
      recentRepos = await commands.listRecentRepos(3);
    } catch (e) {
      // Fail silently - recent repos are optional
    }
  });

  async function checkIfMonorepo(repo: string) {
    if (!repo) {
      isMonorepo = false;
      return;
    }

    checkingMonorepo = true;
    try {
      const moduleCount = await commands.checkMonorepoModules(repo);
      isMonorepo = moduleCount >= 20;
    } catch (e) {
      isMonorepo = false;
    } finally {
      checkingMonorepo = false;
    }
  }

  $effect(() => {
    if (selectedRepo) {
      checkIfMonorepo(selectedRepo);
    } else {
      isMonorepo = false;
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
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      handleCreate();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      handleCreate();
    } else if (recentRepos.length > 0 && !selectedRepo) {
      const num = parseInt(e.key);
      if (num >= 1 && num <= recentRepos.length) {
        e.preventDefault();
        const recent = recentRepos[num - 1];
        selectedRepo = recent.githubRepo;
        if (recent.subpath) {
          subpath = recent.subpath;
        }
      }
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="new-project-form">
  <div class="form-group">
    <label for="project-name">Name</label>
    <input
      bind:value={name}
      id="project-name"
      type="text"
      placeholder="e.g., Add dark mode, Fix login bug"
      disabled={saving}
      autocomplete="off"
      use:focus
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
      >Repository <span class="field-badge optional">Optional</span></label
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
          {#each recentRepos as recent, i}
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
              <div class="keyboard-shortcut">
                <Command size={12} class="command-icon" />
                <span class="shortcut-number">{i + 1}</span>
              </div>
            </button>
          {/each}
        </div>
      {/if}
    {/if}
  </div>

  {#if selectedRepo}
    <div class="form-group">
      <label for="project-subpath"
        >Subpath
        <span class="field-badge {isMonorepo ? 'recommended' : 'optional'}"
          >{isMonorepo ? 'Recommended' : 'Optional'}</span
        ></label
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

  <div class="actions" class:full-width={!onCancel}>
    {#if onCancel}
      <button class="cancel-button" onclick={onCancel} disabled={saving}>Cancel</button>
    {/if}
    <button class="create-button" onclick={handleCreate} disabled={saving || !name.trim()}>
      {saving ? 'Creating...' : 'Create Project'}
    </button>
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
  .new-project-form {
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

  .field-badge {
    display: inline-block;
    font-size: 9px;
    font-weight: 600;
    padding: 2px 6px;
    border-radius: 3px;
    margin-left: 6px;
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .field-badge.optional {
    background-color: var(--bg-hover);
    color: var(--text-faint);
  }

  .field-badge.recommended {
    background-color: var(--ui-accent);
    color: var(--bg-deepest);
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
    background: var(--bg-primary);
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
    background: var(--bg-primary);
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

  .keyboard-shortcut {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 2px 6px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    flex-shrink: 0;
  }

  :global(.command-icon) {
    color: var(--text-muted);
  }

  .shortcut-number {
    font-size: var(--size-xs);
    font-weight: 500;
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

  .actions.full-width .create-button {
    width: 100%;
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
