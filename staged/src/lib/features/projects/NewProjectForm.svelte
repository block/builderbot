<!--
  NewProjectForm.svelte - Reusable project creation form

  Contains the form fields, state, and logic for creating a new project.
  Used inside NewProjectModal (as a dialog) and SplashScreen (inline).
-->
<script lang="ts">
  import { GitBranch, Plus, Monitor, Cloud, Command } from 'lucide-svelte';
  import type { Project, RecentRepo } from '../../types';
  import * as commands from '../../commands';
  import FormInput from '../../shared/FormInput.svelte';
  import FormButton from '../../shared/FormButton.svelte';
  import FormToggle from '../../shared/FormToggle.svelte';
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
    if (e.key === 'Enter') {
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
    <FormInput
      bind:value={name}
      id="project-name"
      placeholder="e.g., Add dark mode, Fix login bug"
      disabled={saving}
      autocomplete="off"
      autofocus
    />
  </div>

  <div class="form-group">
    <div class="field-label">Location</div>
    <FormToggle
      bind:value={location}
      options={[
        { value: 'local', label: 'Local', icon: Monitor },
        { value: 'remote', label: 'Remote', icon: Cloud },
      ]}
      disabled={saving}
    />
  </div>

  <div class="form-group">
    <label for="project-repo-select">Repository</label>
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
      <FormButton
        variant="secondary"
        class="select-repo-button"
        onclick={() => (showRepoPicker = true)}
      >
        <Plus size={14} />
        Select repository
      </FormButton>
      {#if recentRepos.length > 0}
        <div class="recent-repos-section">
          {#each recentRepos as recent, i}
            <FormButton
              variant="ghost"
              class="recent-repo-btn"
              onclick={() => {
                selectedRepo = recent.githubRepo;
                if (recent.subpath) {
                  subpath = recent.subpath;
                }
              }}
            >
              <GitBranch size={12} />
              <span class="recent-repo-name">
                {recent.githubRepo}{#if recent.subpath}<span class="recent-repo-subpath"
                    >/{recent.subpath}</span
                  >{/if}
              </span>
              <span class="keyboard-shortcut">
                <Command size={10} />
                {i + 1}
              </span>
            </FormButton>
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
      <FormInput
        bind:value={subpath}
        id="project-subpath"
        placeholder="e.g., packages/frontend"
        disabled={saving}
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        spellcheck={false}
      />
    </div>
  {/if}

  {#if error}
    <div class="error-message">{error}</div>
  {/if}

  <div class="actions">
    {#if onCancel}
      <FormButton onclick={onCancel} disabled={saving}>Cancel</FormButton>
    {/if}
    <FormButton
      variant="primary"
      class={!onCancel ? 'full-width-btn' : ''}
      onclick={handleCreate}
      disabled={saving || !name.trim()}
    >
      {saving ? 'Creating...' : 'Create Project'}
    </FormButton>
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

  :global(.select-repo-button) {
    width: 100%;
    justify-content: flex-start;
  }

  .repo-info {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 36px;
    padding: 8px 12px;
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

  .change-button {
    border: 1px solid var(--border-muted);
    border-radius: 7px;
    background: transparent;
    color: var(--text-muted);
    padding: 6px 10px;
    cursor: pointer;
  }

  .change-button:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
    background: var(--bg-hover);
  }

  .recent-repos-section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  :global(.recent-repo-btn) {
    width: 100%;
    justify-content: flex-start;
  }

  .recent-repo-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: left;
  }

  .recent-repo-subpath {
    color: var(--text-faint);
  }

  .keyboard-shortcut {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    padding: 2px 5px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-faint);
    font-size: var(--size-xs);
    flex-shrink: 0;
    line-height: 1;
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

  :global(.full-width-btn) {
    width: 100%;
  }
</style>
