<!--
  NewProjectForm.svelte - Reusable project creation form

  Contains the form fields, state, and logic for creating a new project.
  Used inside NewProjectModal (as a dialog) and SplashScreen (inline).
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { slide } from 'svelte/transition';
  import { GitBranch, Monitor, Cloud, X, Clock, Command } from 'lucide-svelte';
  import type { Project, RecentRepo } from '../../types';
  import * as commands from '../../api/commands';
  import FormInput from '../../shared/FormInput.svelte';
  import FormButton from '../../shared/FormButton.svelte';
  import FormToggle from '../../shared/FormToggle.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import RepoSearchInput from './RepoSearchInput.svelte';
  import SubpathInput from './SubpathInput.svelte';
  import type { SubpathInputApi } from './SubpathInput.svelte';
  import BranchPicker, { type BranchSelection } from './BranchPicker.svelte';
  import type { RepoSelection } from '../../shared/githubUrl';
  import { parseGitHubUrl } from '../../shared/githubUrl';
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

  let saving = $state(false);
  let error = $state<string | null>(null);
  let isMonorepo = $state(false);
  let checkingMonorepo = $state(false);
  let subpathApi = $state<SubpathInputApi | undefined>(undefined);
  let recentRepos = $state<RecentRepo[]>([]);

  onMount(async () => {
    try {
      recentRepos = await commands.listRecentRepos(9);
    } catch {
      // Silently ignore — recents are a convenience, not critical
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
      if (selectedRepo && subpath.trim() && subpathApi) {
        const isValid = await subpathApi.waitForValidation();
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
        prNumber
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
    // ⌘1-⌘9: select a recent repo
    if (e.metaKey && e.key >= '1' && e.key <= '9') {
      const idx = parseInt(e.key) - 1;
      if (idx < recentRepos.length) {
        e.preventDefault();
        const recent = recentRepos[idx];
        handleRepoSelected({
          nameWithOwner: recent.githubRepo,
          subpath: recent.subpath ?? undefined,
        });
      }
      return;
    }

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
      handleRepoSelected(parsed);
      // Blur the name field so focus doesn't jump into the newly-revealed BranchPicker
      (document.activeElement as HTMLElement | null)?.blur();
    }
  }

  let pendingPrNumber = $state<number | null>(null);
  let pendingBranchName = $state<string | null>(null);

  function handleRepoSelected(selection: RepoSelection) {
    selectedRepo = selection.nameWithOwner;
    subpath = selection.subpath ?? '';
    pendingPrNumber = selection.prNumber ?? null;
    pendingBranchName = selection.branchName ?? null;
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

  <div class="form-group">
    <label for="project-repo-select"
      >Repository
      {#if location === 'remote' && !selectedRepo}
        <span class="field-badge required">Required</span>
      {/if}</label
    >
    {#if selectedRepo}
      <div class="repo-info">
        <GitBranch size={14} class="repo-info-icon" />
        <div class="repo-details">
          <span class="repo-name">{selectedRepo}</span>
        </div>
        <button
          class="clear-button"
          onclick={() => {
            selectedRepo = null;
            subpath = '';
            branchName = '';
            matchedPr = null;
            pendingPrNumber = null;
            pendingBranchName = null;
          }}
        >
          <X size={14} />
        </button>
      </div>
    {:else}
      <RepoSearchInput onSelect={handleRepoSelected} disabled={saving} />
    {/if}

    {#if !selectedRepo && recentRepos.length > 0}
      <div class="recent-repos" transition:slide={{ duration: 150 }}>
        {#each recentRepos.slice(0, 5) as recent, i}
          <button
            class="recent-repo-item"
            onclick={() =>
              handleRepoSelected({
                nameWithOwner: recent.githubRepo,
                subpath: recent.subpath ?? undefined,
              })}
          >
            <Clock size={12} class="recent-repo-icon" />
            <span class="recent-repo-label">
              <RepoLabel githubRepo={recent.githubRepo} subpath={recent.subpath} />
            </span>
            <span class="recent-repo-shortcut">
              <Command size={9} />
              {i + 1}
            </span>
          </button>
        {/each}
      </div>
    {/if}
  </div>

  {#if selectedRepo}
    <div class="form-group" transition:slide={{ duration: 150 }}>
      <label for="project-subpath"
        >Subpath
        <span class="field-badge {isMonorepo ? 'recommended' : 'optional'}"
          >{isMonorepo ? 'Recommended' : 'Optional'}</span
        ></label
      >
      <SubpathInput
        bind:value={subpath}
        repo={selectedRepo}
        disabled={saving}
        bind:api={subpathApi}
      />
    </div>

    <div class="form-group" transition:slide={{ duration: 150 }}>
      <label for="project-branch"
        >PR or Branch
        <span class="field-badge {isNewBranch ? 'new-branch' : 'optional'}"
          >{isNewBranch ? 'New branch' : 'Optional'}</span
        ></label
      >
      <BranchPicker
        bind:value={branchName}
        bind:isNewBranch
        bind:matchedPr
        bind:initialPrNumber={pendingPrNumber}
        bind:initialBranchName={pendingBranchName}
        repo={selectedRepo}
        disabled={saving}
        onSelect={handleBranchSelected}
      />
    </div>
  {/if}

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

  .field-badge.required {
    background-color: var(--ui-danger);
    color: var(--bg-deepest);
  }

  .field-badge.new-branch {
    background-color: var(--ui-accent);
    color: var(--bg-deepest);
  }

  .repo-info {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 42px;
    padding: 10px 8px 10px 14px;
    background: var(--text-primary);
    color: var(--bg-deepest);
    border: 1.5px solid var(--text-primary);
    border-radius: 10px;
  }

  .repo-info :global(.repo-info-icon) {
    color: var(--bg-deepest);
  }

  .repo-details {
    min-width: 0;
    flex: 1;
  }

  .repo-name {
    font-size: var(--size-sm);
    color: var(--bg-deepest);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .clear-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--bg-chrome);
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      color 0.15s ease;
  }

  .clear-button:hover {
    color: var(--bg-deepest);
    background: rgba(0, 0, 0, 0.1);
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

  .recent-repos {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-top: 2px;
  }

  .recent-repo-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 5px 8px;
    background: none;
    border: none;
    border-radius: 6px;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.15s ease;
    font-family: inherit;
  }

  .recent-repo-item:hover {
    background-color: var(--bg-hover);
  }

  .recent-repo-item :global(.recent-repo-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .recent-repo-label {
    flex: 1;
    min-width: 0;
    font-size: var(--size-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .recent-repo-shortcut {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    padding: 1px 4px;
    background: var(--bg-hover);
    border-radius: 3px;
    color: var(--text-faint);
    font-size: 10px;
    flex-shrink: 0;
    line-height: 1;
  }
</style>
