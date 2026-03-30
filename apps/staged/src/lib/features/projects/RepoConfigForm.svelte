<!--
  RepoConfigForm.svelte - Repo configuration form fields

  Contains repository search, selected repo display, recent repos list,
  subpath input with monorepo detection, and branch/PR picker.
  Extracted from NewProjectForm for reuse in Add Repo flows.
-->
<script lang="ts" module>
  export type { BranchSelection } from './BranchPicker.svelte';
</script>

<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { slide } from 'svelte/transition';
  import { GitBranch, X, Clock, Command } from 'lucide-svelte';
  import type { RecentRepo, PullRequest } from '../../types';
  import * as commands from '../../api/commands';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import RepoSearchInput from './RepoSearchInput.svelte';
  import SubpathInput from './SubpathInput.svelte';
  import type { SubpathInputApi } from './SubpathInput.svelte';
  import BranchPicker, { type BranchSelection } from './BranchPicker.svelte';
  import type { RepoSelection } from '../../shared/githubUrl';

  interface Props {
    // Bindable state
    selectedRepo?: string | null;
    subpath?: string;
    branchName?: string;
    isNewBranch?: boolean;
    matchedPr?: PullRequest | null;
    /** Pre-fetched default branch for the selected repo (avoids slow API call during project creation). */
    defaultBranch?: string | null;

    // Config
    disabled?: boolean;
    excludeRepos?: Set<string>;
    autofocus?: boolean;

    // Optional callbacks
    onBranchSelected?: (selection: BranchSelection) => void;

    // Show "Required" badge on Repository label (for remote location in NewProjectForm)
    repoRequired?: boolean;

    // Exposed API for parent validation and programmatic repo selection
    api?: {
      waitForSubpathValidation: () => Promise<boolean>;
      selectRepo: (selection: RepoSelection) => void;
    };
  }

  let {
    selectedRepo = $bindable(null),
    subpath = $bindable(''),
    branchName = $bindable(''),
    isNewBranch = $bindable(false),
    matchedPr = $bindable(null),
    defaultBranch = $bindable(null),
    disabled = false,
    excludeRepos,
    autofocus = false,
    repoRequired = false,
    onBranchSelected,
    api = $bindable<
      | {
          waitForSubpathValidation: () => Promise<boolean>;
          selectRepo: (selection: RepoSelection) => void;
        }
      | undefined
    >(undefined),
  }: Props = $props();

  let isMonorepo = $state(false);
  let checkingMonorepo = $state(false);
  let subpathApi = $state<SubpathInputApi | undefined>(undefined);
  let recentRepos = $state<RecentRepo[]>([]);
  const SLIDE_DURATION = 150;

  /** BranchPicker is expensive to mount in WebKit — defer it until after the
   *  initial layout change paints so the UI feels responsive. */
  let showBranchPicker = $state(false);
  let branchPickerTimer: ReturnType<typeof setTimeout> | null = null;

  let pendingPrNumber = $state<number | null>(null);
  let pendingBranchName = $state<string | null>(null);

  onMount(async () => {
    try {
      recentRepos = await commands.listRecentRepos(9);
    } catch {
      // Silently ignore — recents are a convenience, not critical
    }
  });

  onDestroy(() => {
    if (branchPickerTimer) clearTimeout(branchPickerTimer);
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
      prefetchDefaultBranch(selectedRepo);
    } else {
      isMonorepo = false;
      defaultBranch = null;
    }
  });

  async function prefetchDefaultBranch(repo: string) {
    try {
      defaultBranch = await commands.detectDefaultBranch(repo);
    } catch {
      defaultBranch = null;
    }
  }

  // Expose validation API to parent
  $effect(() => {
    api = {
      waitForSubpathValidation: subpathApi
        ? () => subpathApi!.waitForValidation()
        : () => Promise.resolve(true),
      selectRepo: handleRepoSelected,
    };
  });

  let filteredRecentRepos = $derived(
    excludeRepos ? recentRepos.filter((r) => !excludeRepos.has(r.githubRepo)) : recentRepos
  );

  function handleRepoSelected(selection: RepoSelection) {
    if (branchPickerTimer) clearTimeout(branchPickerTimer);
    showBranchPicker = false;
    selectedRepo = selection.nameWithOwner;
    subpath = selection.subpath ?? '';
    pendingPrNumber = selection.prNumber ?? null;
    pendingBranchName = selection.branchName ?? null;
    // Mount BranchPicker after the slide animation completes.
    // BranchPicker is expensive to mount in WebKit, so deferring it
    // keeps the initial repo selection feeling responsive.
    branchPickerTimer = setTimeout(() => {
      showBranchPicker = true;
    }, SLIDE_DURATION + 50);
  }

  function handleKeydown(e: KeyboardEvent) {
    // ⌘1-⌘9: select a recent repo
    if (e.metaKey && e.key >= '1' && e.key <= '9') {
      const idx = parseInt(e.key) - 1;
      if (idx < filteredRecentRepos.length) {
        e.preventDefault();
        const recent = filteredRecentRepos[idx];
        handleRepoSelected({
          nameWithOwner: recent.githubRepo,
          subpath: recent.subpath ?? undefined,
        });
      }
      return;
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="form-group">
  <label for="project-repo-select"
    >Repository
    {#if repoRequired && !selectedRepo}
      <span class="field-badge required">Required</span>
    {/if}</label
  >
  {#if selectedRepo}
    <div class="repo-info" class:disabled>
      <GitBranch size={14} class="repo-info-icon" />
      <div class="repo-details">
        <span class="repo-name">{selectedRepo}</span>
      </div>
      <button
        class="clear-button"
        onclick={() => {
          if (branchPickerTimer) clearTimeout(branchPickerTimer);
          showBranchPicker = false;
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
    <RepoSearchInput onSelect={handleRepoSelected} {disabled} {excludeRepos} />
  {/if}

  {#if !selectedRepo && filteredRecentRepos.length > 0}
    <div class="recent-repos" out:slide={{ duration: SLIDE_DURATION }}>
      {#each filteredRecentRepos.slice(0, 5) as recent, i}
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
  <div class="form-group" transition:slide={{ duration: SLIDE_DURATION }}>
    <label for="project-subpath"
      >Subpath
      <span class="field-badge {isMonorepo ? 'recommended' : 'optional'}"
        >{isMonorepo ? 'Recommended' : 'Optional'}</span
      ></label
    >
    <SubpathInput bind:value={subpath} repo={selectedRepo} {disabled} bind:api={subpathApi} />
  </div>

  <div class="form-group" transition:slide={{ duration: SLIDE_DURATION }}>
    <label for="project-branch"
      >PR or Branch
      <span class="field-badge {isNewBranch ? 'new-branch' : 'optional'}"
        >{isNewBranch ? 'New branch' : 'Optional'}</span
      ></label
    >
    {#if showBranchPicker}
      <BranchPicker
        bind:value={branchName}
        bind:isNewBranch
        bind:matchedPr
        bind:initialPrNumber={pendingPrNumber}
        bind:initialBranchName={pendingBranchName}
        repo={selectedRepo}
        {disabled}
        onSelect={onBranchSelected}
      />
    {:else}
      <input
        class="branch-picker-placeholder"
        type="text"
        placeholder="Search PRs or branches…"
        readonly
        tabindex="-1"
      />
    {/if}
  </div>
{/if}

<style>
  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  label {
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

  .repo-info.disabled {
    opacity: 0.6;
    pointer-events: none;
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

  .branch-picker-placeholder {
    width: 100%;
    min-height: 42px;
    border: 1.5px solid var(--border-muted);
    border-radius: 10px;
    background: transparent;
    color: var(--text-primary);
    padding: 10px 14px;
    font-size: var(--size-md);
    font-family: inherit;
    outline: none;
    box-sizing: border-box;
  }

  .branch-picker-placeholder::placeholder {
    color: var(--text-faint);
  }
</style>
