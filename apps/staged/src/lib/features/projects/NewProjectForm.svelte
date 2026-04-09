<!--
  NewProjectForm.svelte - Reusable project creation form

  Contains the form fields, state, and logic for creating a new project.
  Used inside NewProjectModal (as a dialog) and SplashScreen (inline).
-->
<script lang="ts">
  import { slide } from 'svelte/transition';
  import { Monitor, Cloud } from 'lucide-svelte';
  import type { Project } from '../../types';
  import * as commands from '../../api/commands';
  import FormInput from '../../shared/FormInput.svelte';
  import FormButton from '../../shared/FormButton.svelte';
  import FormToggle from '../../shared/FormToggle.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoConfigForm from './RepoConfigForm.svelte';
  import type { BranchSelection } from './RepoConfigForm.svelte';
  import { parseGitHubUrl } from '../../shared/githubUrl';
  import type { RepoSelection } from '../../shared/githubUrl';
  import type { PullRequest } from '../../types';

  interface Props {
    onCreated: (project: Project) => void;
    onCancel?: () => void;
    name?: string;
    location?: 'local' | 'remote';
    selectedRepo?: string | null;
    subpath?: string;
  }

  let {
    onCreated,
    onCancel,
    name = $bindable(''),
    location = $bindable('local'),
    selectedRepo = $bindable(null),
    subpath = $bindable(''),
  }: Props = $props();

  let branchName = $state('');
  let isNewBranch = $state(false);
  let matchedPr = $state<PullRequest | null>(null);
  let defaultBranch = $state<string | null>(null);

  let saving = $state(false);
  let error = $state<string | null>(null);
  let repoConfigApi = $state<
    | {
        waitForSubpathValidation: () => Promise<boolean>;
        selectRepo: (selection: RepoSelection) => void;
      }
    | undefined
  >(undefined);

  // Clear error when user edits the subpath
  $effect(() => {
    subpath;
    error = null;
  });

  let repoMissing = $derived(location === 'remote' && !selectedRepo);
  let canCreate = $derived(!!name.trim() && !saving && !repoMissing);

  async function handleCreate() {
    if (!canCreate) return;

    saving = true;
    error = null;

    try {
      // If there's a subpath and a repo, validate before creating
      if (selectedRepo && subpath.trim() && repoConfigApi) {
        const isValid = await repoConfigApi.waitForSubpathValidation();
        if (!isValid) {
          error = 'Invalid path in repo';
          saving = false;
          return;
        }
      }

      const normalizedSubpath = selectedRepo
        ? subpath.trim().replace(/^\/+|\/+$/g, '') || undefined
        : undefined;

      const normalizedBranch = selectedRepo ? branchName.trim() || undefined : undefined;
      const prNumber = matchedPr?.number ?? undefined;

      const project = await commands.createProject(
        name.trim(),
        location,
        selectedRepo ?? undefined,
        normalizedSubpath,
        normalizedBranch,
        prNumber,
        defaultBranch ?? undefined
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
      const target = e.target as HTMLElement;
      if (target.closest('.repo-search-wrapper')) return;
      // Don't submit if a suggestion is highlighted in the subpath dropdown
      if (target.closest('.subpath-input-wrapper')) return;
      // Don't submit if a suggestion is highlighted in the branch picker dropdown
      if (target.closest('.branch-picker-wrapper')) return;
      e.preventDefault();
      handleCreate();
    }
  }

  /** Derive a human-friendly project name from a branch name like "feat/dark-mode". */
  function nameFromBranch(branch: string): string {
    const last = branch.split('/').pop() ?? branch;
    if (!last) return branch;
    const spaced = last.replace(/[-_]/g, ' ');
    return spaced.charAt(0).toUpperCase() + spaced.slice(1);
  }

  function handleBranchSelected(selection: BranchSelection) {
    if (!name.trim()) {
      name = selection.kind === 'pr' ? selection.label : nameFromBranch(selection.branchName);
    }
  }

  function handleNameInput() {
    const parsed = parseGitHubUrl(name);
    if (parsed) {
      name = '';
      if (repoConfigApi) {
        repoConfigApi.selectRepo(parsed);
      }
      // Blur the name field so focus doesn't jump into the newly-revealed BranchPicker
      (document.activeElement as HTMLElement | null)?.blur();
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
      autocorrect="off"
      autocapitalize="off"
      spellcheck={false}
      autofocus
      oninput={handleNameInput}
    />
  </div>

  <div class="form-group">
    <div class="field-label">Location</div>
    <FormToggle
      bind:value={location}
      options={[
        {
          value: 'local',
          label: 'Local',
          description: 'Run agents on your machine',
          icon: Monitor,
        },
        {
          value: 'remote',
          label: 'Remote',
          description: 'Run agents in the cloud',
          icon: Cloud,
        },
      ]}
      disabled={saving}
    />
  </div>

  <RepoConfigForm
    bind:selectedRepo
    bind:subpath
    bind:branchName
    bind:isNewBranch
    bind:matchedPr
    bind:defaultBranch
    bind:api={repoConfigApi}
    disabled={saving}
    repoRequired={location === 'remote'}
    onBranchSelected={handleBranchSelected}
  />

  {#if error}
    <div class="error-message" transition:slide={{ duration: 150 }}>{error}</div>
  {/if}

  <div class="actions">
    {#if onCancel}
      <FormButton onclick={onCancel} disabled={saving}>Cancel</FormButton>
    {/if}
    <FormButton
      variant="primary"
      class={!onCancel ? 'full-width-btn' : ''}
      onclick={handleCreate}
      disabled={!canCreate}
    >
      {#if saving}
        <span class="button-content">
          <Spinner size={14} />
          <span>Creating...</span>
        </span>
      {:else}
        Create Project
      {/if}
    </FormButton>
  </div>
</div>

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

  .button-content {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
</style>
