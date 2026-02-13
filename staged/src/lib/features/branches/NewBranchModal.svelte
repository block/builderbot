<!--
  NewBranchModal.svelte - Create a new branch (local worktree or remote Blox workspace)

  Two modes toggled by a segmented control:
  - Local: branch name + base branch picker → creates git worktree
  - Remote: branch name → starts a Blox workspace

  Optionally import from a GitHub PR or issue to auto-populate the branch name
  and (for PRs) set the base branch.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    X,
    GitBranch,
    Search,
    ChevronsUpDown,
    Check,
    Monitor,
    Cloud,
    Github,
    GitPullRequest,
    CircleDot,
  } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import type { Branch, BranchRef, Project, BranchType, PullRequest, Issue } from '../../types';
  import * as commands from '../../commands';
  import { sqState } from '../settings/sq.svelte';

  interface Props {
    project: Project;
    onCreated: (branch: Branch) => void;
    onClose: () => void;
  }

  let { project, onCreated, onClose }: Props = $props();

  // Branch type toggle
  let branchType = $state<BranchType>('local');

  // State
  let branchTitle = $state('');
  let creating = $state(false);
  let loading = $state(true);
  let error = $state<string | null>(null);

  // Default branch (detected on mount)
  let detectedDefaultBranch = $state<string | null>(null);
  let selectedBaseBranch = $state<string | null>(null);

  // Base branch picker
  let showBasePicker = $state(false);
  let allBranchRefs = $state<BranchRef[]>([]);
  let baseSearchQuery = $state('');
  let baseSelectedIndex = $state(0);

  let branchInputEl: HTMLInputElement | null = $state(null);
  let baseSearchEl: HTMLInputElement | null = $state(null);

  // GitHub picker state
  let showGithubPicker = $state(false);
  let githubTab = $state<'pr' | 'issue'>('pr');
  let pullRequests = $state<PullRequest[]>([]);
  let issues = $state<Issue[]>([]);
  let githubSearchQuery = $state('');
  let githubError = $state<string | null>(null);
  let githubSelectedIndex = $state(0);
  let prsLoading = $state(false);
  let prsLoaded = $state(false);
  let issuesLoading = $state(false);
  let issuesLoaded = $state(false);

  let githubSearchEl: HTMLInputElement | null = $state(null);

  let effectiveBaseBranch = $derived(selectedBaseBranch ?? detectedDefaultBranch ?? 'main');

  // For remote branches, only show remote-tracking refs (branches that exist on the remote).
  // For local branches, show all refs.
  let availableBranches = $derived.by(() => {
    if (branchType === 'remote') {
      return allBranchRefs.filter((r) => r.isRemote).map((r) => r.name);
    }
    return allBranchRefs.map((r) => r.name);
  });

  let filteredBranches = $derived.by(() => {
    if (!baseSearchQuery) return availableBranches;
    const q = baseSearchQuery.toLowerCase();
    return availableBranches.filter((b) => b.toLowerCase().includes(q));
  });

  // Filtered GitHub items (client-side)
  let filteredPullRequests = $derived.by(() => {
    if (!githubSearchQuery) return pullRequests;
    const q = githubSearchQuery.toLowerCase();
    return pullRequests.filter(
      (pr) =>
        pr.title.toLowerCase().includes(q) ||
        `#${pr.number}`.includes(q) ||
        pr.author.toLowerCase().includes(q)
    );
  });

  let filteredIssues = $derived.by(() => {
    if (!githubSearchQuery) return issues;
    const q = githubSearchQuery.toLowerCase();
    return issues.filter(
      (issue) =>
        issue.title.toLowerCase().includes(q) ||
        `#${issue.number}`.includes(q) ||
        issue.author.toLowerCase().includes(q)
    );
  });

  let currentGithubList = $derived(githubTab === 'pr' ? filteredPullRequests : filteredIssues);
  let githubLoading = $derived(githubTab === 'pr' ? prsLoading : issuesLoading);

  /**
   * Sanitize a branch title into a valid git branch name.
   */
  function sanitizeBranchName(title: string): string {
    return title
      .toLowerCase()
      .replace(/[\s_]+/g, '-')
      .replace(/[~^:?*\[\]\\@{}"'`!#$%&()|<>=+;,]/g, '')
      .replace(/[-.]+/g, '-')
      .replace(/^[-.]+|[-.]+$/g, '');
  }

  let branchName = $derived(sanitizeBranchName(branchTitle));

  const REMOTE_BRANCH_MAX_LENGTH = 32;

  /** For remote branches: replace slashes with dashes and enforce max length. */
  let effectiveBranchName = $derived.by(() => {
    if (branchType !== 'remote') return branchName;
    let name = branchName
      .replace(/\/+/g, '-')
      .replace(/-+/g, '-')
      .replace(/^-+|-+$/g, '');
    if (name.length > REMOTE_BRANCH_MAX_LENGTH) {
      name = name.slice(0, REMOTE_BRANCH_MAX_LENGTH).replace(/-+$/, '');
    }
    return name;
  });

  /** Generate a workspace name from the branch name. */
  function workspaceName(name: string): string {
    if (!name) return '';
    // Prefix with repo name for uniqueness
    const repo = repoName(project.githubRepo);
    return `${repo}-${name}`;
  }

  onMount(async () => {
    // Fetch default branch and branch list in parallel
    const [defaultBranchResult, branchRefsResult] = await Promise.allSettled([
      commands.detectDefaultBranch(project.githubRepo),
      commands.listGitBranches(project.githubRepo),
    ]);

    detectedDefaultBranch =
      defaultBranchResult.status === 'fulfilled' ? defaultBranchResult.value : 'main';
    allBranchRefs = branchRefsResult.status === 'fulfilled' ? branchRefsResult.value : [];

    loading = false;

    // Fire-and-forget: prune stale remote-tracking refs in the background.
    // Once done, silently refresh the branch list so any stale refs disappear.
    commands
      .pruneRemoteRefs(project.githubRepo)
      .then(() => commands.listGitBranches(project.githubRepo))
      .then((refs) => {
        allBranchRefs = refs;
      })
      .catch(() => {
        // Best-effort — ignore errors (e.g. no network, no remote configured)
      });
  });

  // Focus branch input
  $effect(() => {
    if (branchInputEl && !showBasePicker && !showGithubPicker) {
      branchInputEl.focus();
    }
  });

  // Focus base search when picker opens
  $effect(() => {
    if (showBasePicker && baseSearchEl) {
      baseSearchEl.focus();
    }
  });

  // Focus github search when picker opens
  $effect(() => {
    if (showGithubPicker && githubSearchEl) {
      githubSearchEl.focus();
    }
  });

  function toggleBasePicker() {
    showBasePicker = !showBasePicker;
    baseSearchQuery = '';
    baseSelectedIndex = 0;
  }

  function selectBaseBranch(branch: string) {
    selectedBaseBranch = branch;
    showBasePicker = false;
    baseSearchQuery = '';
    baseSelectedIndex = 0;
  }

  function loadPrs() {
    if (prsLoaded || prsLoading) return;
    prsLoading = true;
    githubError = null;
    commands
      .listPullRequests(project.githubRepo)
      .then((result) => {
        pullRequests = result;
        prsLoaded = true;
      })
      .catch((e) => {
        githubError = typeof e === 'string' ? e : String(e);
      })
      .finally(() => {
        prsLoading = false;
      });
  }

  function loadIssues() {
    if (issuesLoaded || issuesLoading) return;
    issuesLoading = true;
    githubError = null;
    commands
      .listIssues(project.githubRepo)
      .then((result) => {
        issues = result;
        issuesLoaded = true;
      })
      .catch((e) => {
        githubError = typeof e === 'string' ? e : String(e);
      })
      .finally(() => {
        issuesLoading = false;
      });
  }

  function openGithubPicker() {
    showGithubPicker = true;
    githubTab = 'pr';
    githubSearchQuery = '';
    githubSelectedIndex = 0;
    githubError = null;
    loadPrs();
  }

  function switchGithubTab(tab: 'pr' | 'issue') {
    githubTab = tab;
    githubSelectedIndex = 0;
    githubSearchQuery = '';
    githubError = null;

    if (tab === 'issue') {
      loadIssues();
    } else {
      loadPrs();
    }
  }

  function selectPullRequest(pr: PullRequest) {
    branchTitle = `pr-${pr.number}-${pr.title}`;
    // Auto-set base branch to the PR's target
    selectedBaseBranch = pr.baseRef;
    showGithubPicker = false;
  }

  function selectIssue(issue: Issue) {
    branchTitle = `issue-${issue.number}-${issue.title}`;
    showGithubPicker = false;
  }

  function selectGithubItem(index: number) {
    if (githubTab === 'pr') {
      const pr = filteredPullRequests[index];
      if (pr) selectPullRequest(pr);
    } else {
      const issue = filteredIssues[index];
      if (issue) selectIssue(issue);
    }
  }

  function formatTimeAgo(dateStr: string): string {
    const date = new Date(dateStr);
    const now = Date.now();
    const diffMs = now - date.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    if (diffMins < 60) return `${diffMins}m ago`;
    const diffHours = Math.floor(diffMins / 60);
    if (diffHours < 24) return `${diffHours}h ago`;
    const diffDays = Math.floor(diffHours / 24);
    return `${diffDays}d ago`;
  }

  async function handleCreate() {
    if (!effectiveBranchName.trim() || creating) return;

    creating = true;
    error = null;

    try {
      if (branchType === 'local') {
        const baseBranch = selectedBaseBranch ?? undefined;
        // Fast: creates DB record only (no git worktree yet)
        const branch = await commands.createBranch(
          project.id,
          effectiveBranchName.trim(),
          baseBranch
        );

        // Dismiss immediately — the card will show "Creating worktree…" state.
        // ProjectHome handles worktree setup + prerun actions in the background.
        onCreated(branch);
      } else {
        const wsName = workspaceName(effectiveBranchName.trim());
        const branch = await commands.createRemoteBranch(
          project.id,
          effectiveBranchName.trim(),
          wsName,
          selectedBaseBranch ?? undefined
        );

        // Dismiss immediately — the card will show "Provisioning…" state
        onCreated(branch);

        // Kick off workspace provisioning in the background
        commands.startWorkspace(branch.id).catch((e) => {
          console.error('[NewBranchModal] Failed to start workspace:', e);
        });
      }
    } catch (e) {
      if (typeof e === 'string') {
        error = e;
      } else if (e instanceof Error) {
        error = e.message;
      } else {
        error = String(e);
      }
      creating = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      if (showGithubPicker) {
        showGithubPicker = false;
        githubSearchQuery = '';
      } else if (showBasePicker) {
        showBasePicker = false;
        baseSearchQuery = '';
      } else {
        onClose();
      }
      return;
    }

    if (showGithubPicker) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        githubSelectedIndex = Math.min(githubSelectedIndex + 1, currentGithubList.length - 1);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        githubSelectedIndex = Math.max(githubSelectedIndex - 1, 0);
      } else if (e.key === 'Enter' && currentGithubList.length > 0) {
        e.preventDefault();
        selectGithubItem(githubSelectedIndex);
      }
    } else if (showBasePicker) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        baseSelectedIndex = Math.min(baseSelectedIndex + 1, filteredBranches.length - 1);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        baseSelectedIndex = Math.max(baseSelectedIndex - 1, 0);
      } else if (e.key === 'Enter' && filteredBranches.length > 0) {
        e.preventDefault();
        selectBaseBranch(filteredBranches[baseSelectedIndex]);
      }
    } else {
      if (e.key === 'Enter' && effectiveBranchName.trim()) {
        e.preventDefault();
        handleCreate();
      }
    }
  }

  function formatBranchName(name: string): string {
    return name.replace(/^origin\//, '');
  }

  function repoName(path: string): string {
    return path.split('/').pop() || path;
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
      <h2>New Branch</h2>
      <button class="close-button" onclick={onClose}>
        <X size={18} />
      </button>
    </div>

    <div class="modal-body">
      {#if loading}
        <div class="loading-state">
          <Spinner size={20} />
          <span>Loading branches…</span>
        </div>
      {:else}
        <!-- Branch type toggle (only shown when sq CLI is available) -->
        {#if sqState.available}
          <div class="type-toggle">
            <button
              class="toggle-option"
              class:active={branchType === 'local'}
              onclick={() => {
                branchType = 'local';
                selectedBaseBranch = null;
              }}
            >
              <Monitor size={14} />
              Local
            </button>
            <button
              class="toggle-option"
              class:active={branchType === 'remote'}
              onclick={() => {
                branchType = 'remote';
                selectedBaseBranch = null;
              }}
            >
              <Cloud size={14} />
              Remote
            </button>
          </div>
        {/if}

        <div class="selected-info">
          <div class="info-row">
            <GitBranch size={14} />
            <span class="info-label">Repository:</span>
            <span class="info-value">{repoName(project.githubRepo)}</span>
          </div>
          <button class="info-row base-row" onclick={toggleBasePicker}>
            <GitBranch size={14} class="base-icon" />
            <span class="info-label">Base:</span>
            <span class="info-value">{formatBranchName(effectiveBaseBranch)}</span>
            <ChevronsUpDown size={12} class="base-chevron" />
          </button>
        </div>

        {#if showBasePicker}
          <!-- Base branch picker -->
          <div class="base-picker">
            <div class="base-search-container">
              <Search size={14} class="search-icon" />
              <input
                bind:this={baseSearchEl}
                bind:value={baseSearchQuery}
                type="text"
                placeholder="Search branches..."
                class="base-search-input"
              />
            </div>
            <div class="base-list">
              {#each filteredBranches as branch, index (branch)}
                <button
                  class="base-item"
                  class:selected={index === baseSelectedIndex}
                  onclick={() => selectBaseBranch(branch)}
                >
                  <span class="base-item-name">{branch}</span>
                  {#if branch === effectiveBaseBranch}
                    <Check size={14} class="check-icon" />
                  {/if}
                </button>
              {/each}
              {#if filteredBranches.length === 0}
                <div class="base-empty">No branches found</div>
              {/if}
            </div>
          </div>
        {:else if showGithubPicker}
          <!-- GitHub PR/Issue picker -->
          <div class="github-picker">
            <div class="github-tabs">
              <button
                class="github-tab"
                class:active={githubTab === 'pr'}
                onclick={() => switchGithubTab('pr')}
              >
                <GitPullRequest size={13} />
                PRs
              </button>
              <button
                class="github-tab"
                class:active={githubTab === 'issue'}
                onclick={() => switchGithubTab('issue')}
              >
                <CircleDot size={13} />
                Issues
              </button>
            </div>
            <div class="github-search-container">
              <Search size={14} class="search-icon" />
              <input
                bind:this={githubSearchEl}
                bind:value={githubSearchQuery}
                oninput={() => (githubSelectedIndex = 0)}
                type="text"
                placeholder="Filter..."
                class="github-search-input"
              />
            </div>
            <div class="github-list">
              {#if githubLoading}
                <div class="github-loading">
                  <Spinner size={16} />
                  <span>Loading...</span>
                </div>
              {:else if githubError}
                <div class="github-error">{githubError}</div>
              {:else if githubTab === 'pr'}
                {#each filteredPullRequests as pr, index (pr.number)}
                  <button
                    class="github-item"
                    class:selected={index === githubSelectedIndex}
                    onclick={() => selectPullRequest(pr)}
                  >
                    <div class="github-item-main">
                      <span class="github-item-number">#{pr.number}</span>
                      <span class="github-item-title">{pr.title}</span>
                      {#if pr.draft}
                        <span class="github-draft-badge">Draft</span>
                      {/if}
                    </div>
                    <div class="github-item-meta">
                      @{pr.author} &middot; {formatTimeAgo(pr.updatedAt)}
                    </div>
                  </button>
                {/each}
                {#if filteredPullRequests.length === 0}
                  <div class="github-empty">No pull requests found</div>
                {/if}
              {:else}
                {#each filteredIssues as issue, index (issue.number)}
                  <button
                    class="github-item"
                    class:selected={index === githubSelectedIndex}
                    onclick={() => selectIssue(issue)}
                  >
                    <div class="github-item-main">
                      <span class="github-item-number">#{issue.number}</span>
                      <span class="github-item-title">{issue.title}</span>
                    </div>
                    <div class="github-item-meta">
                      @{issue.author} &middot; {formatTimeAgo(issue.updatedAt)}
                    </div>
                  </button>
                {/each}
                {#if filteredIssues.length === 0}
                  <div class="github-empty">No issues found</div>
                {/if}
              {/if}
            </div>
          </div>
        {:else}
          <div class="input-group">
            <label for="branch-title">Branch name</label>
            <input
              bind:this={branchInputEl}
              bind:value={branchTitle}
              id="branch-title"
              type="text"
              placeholder={branchType === 'local' ? 'Fix login issue' : 'Add user auth'}
              class="branch-input"
              autocomplete="off"
              autocorrect="off"
              autocapitalize="off"
              spellcheck="false"
            />
            {#if branchTitle && effectiveBranchName !== branchTitle.toLowerCase()}
              <div class="branch-preview">
                <GitBranch size={12} />
                <span>{effectiveBranchName || '...'}</span>
              </div>
            {/if}
            {#if branchType === 'remote' && effectiveBranchName}
              <div class="workspace-preview">
                <Cloud size={12} />
                <span>Workspace: {workspaceName(effectiveBranchName)}</span>
              </div>
            {/if}
          </div>

          <button class="github-import-button" onclick={openGithubPicker}>
            <Github size={14} />
            Import from GitHub...
          </button>

          {#if error}
            <div class="error-message">{error}</div>
          {/if}

          <div class="actions">
            <button class="cancel-button" onclick={onClose}>Cancel</button>
            <button
              class="create-button"
              onclick={handleCreate}
              disabled={!effectiveBranchName || creating}
            >
              {#if creating}
                <Spinner size={14} />
                Creating...
              {:else}
                Create Branch
              {/if}
            </button>
          </div>
        {/if}
      {/if}
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background-color: var(--shadow-overlay);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 10vh;
    z-index: 1000;
  }

  .modal {
    width: 460px;
    max-width: 90vw;
    background-color: var(--bg-primary);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 16px;
    border-bottom: 1px solid var(--border-subtle);
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
    gap: 16px;
  }

  /* Loading state */
  .loading-state {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 32px 16px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  /* Branch type toggle */
  .type-toggle {
    display: flex;
    gap: 2px;
    padding: 3px;
    background-color: var(--bg-hover);
    border-radius: 8px;
  }

  .toggle-option {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 7px 12px;
    background: transparent;
    border: none;
    border-radius: 6px;
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .toggle-option:hover:not(.active) {
    color: var(--text-primary);
  }

  .toggle-option.active {
    background-color: var(--bg-primary);
    color: var(--text-primary);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.08);
  }

  .toggle-option :global(svg) {
    flex-shrink: 0;
  }

  .selected-info {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px;
    background-color: var(--bg-hover);
    border-radius: 6px;
  }

  .info-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-sm);
  }

  .info-row :global(svg) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  :global(.base-icon) {
    color: var(--text-muted) !important;
  }

  .info-label {
    color: var(--text-muted);
  }

  .info-value {
    color: var(--text-primary);
    font-family: 'SF Mono', 'Menlo', monospace;
  }

  .base-row {
    background: transparent;
    border: none;
    border-radius: 4px;
    cursor: pointer;
    margin: -4px -6px;
    padding: 4px 6px;
    transition: background-color 0.15s;
  }

  .base-row:hover {
    background-color: var(--bg-primary);
  }

  :global(.base-chevron) {
    color: var(--text-faint);
    margin-left: auto;
  }

  .base-row:hover :global(.base-chevron) {
    color: var(--text-muted);
  }

  /* Base branch picker */
  .base-picker {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    overflow: hidden;
  }

  .base-search-container {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border-subtle);
  }

  :global(.search-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .base-search-input {
    flex: 1;
    padding: 4px 0;
    background: transparent;
    border: none;
    outline: none;
    font-size: var(--size-sm);
    color: var(--text-primary);
  }

  .base-search-input::placeholder {
    color: var(--text-faint);
  }

  .base-list {
    max-height: 200px;
    overflow-y: auto;
    padding: 4px;
  }

  .base-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 10px;
    background: transparent;
    border: none;
    border-radius: 4px;
    font-size: var(--size-sm);
    color: var(--text-primary);
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .base-item:hover,
  .base-item.selected {
    background-color: var(--bg-hover);
  }

  .base-item-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: 'SF Mono', 'Menlo', monospace;
  }

  :global(.check-icon) {
    color: var(--ui-accent);
    flex-shrink: 0;
  }

  .base-empty {
    padding: 16px;
    text-align: center;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .input-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .input-group label {
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .branch-input {
    padding: 10px 12px;
    background-color: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    font-size: var(--size-md);
    color: var(--text-primary);
    outline: none;
    transition: border-color 0.15s;
  }

  .branch-input:focus {
    border-color: var(--ui-accent);
  }

  .branch-input::placeholder {
    color: var(--text-faint);
  }

  .branch-preview {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .branch-preview :global(svg) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .branch-preview span {
    font-family: 'SF Mono', 'Menlo', monospace;
    color: var(--text-muted);
  }

  .workspace-preview {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 2px;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .workspace-preview :global(svg) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .workspace-preview span {
    font-family: 'SF Mono', 'Menlo', monospace;
  }

  /* GitHub import button */
  .github-import-button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 12px;
    background: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: all 0.15s;
    align-self: flex-start;
  }

  .github-import-button:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
    background-color: var(--bg-hover);
  }

  .github-import-button :global(svg) {
    flex-shrink: 0;
  }

  /* GitHub picker */
  .github-picker {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    overflow: hidden;
  }

  .github-tabs {
    display: flex;
    border-bottom: 1px solid var(--border-subtle);
  }

  .github-tab {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 5px;
    padding: 8px 12px;
    background: transparent;
    border: none;
    border-bottom: 2px solid transparent;
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.15s;
    margin-bottom: -1px;
  }

  .github-tab:hover {
    color: var(--text-primary);
  }

  .github-tab.active {
    color: var(--text-primary);
    border-bottom-color: var(--ui-accent);
  }

  .github-tab :global(svg) {
    flex-shrink: 0;
  }

  .github-search-container {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .github-search-input {
    flex: 1;
    padding: 4px 0;
    background: transparent;
    border: none;
    outline: none;
    font-size: var(--size-sm);
    color: var(--text-primary);
  }

  .github-search-input::placeholder {
    color: var(--text-faint);
  }

  .github-list {
    max-height: 240px;
    overflow-y: auto;
    padding: 4px;
  }

  .github-loading {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 24px 16px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .github-error {
    padding: 16px;
    text-align: center;
    color: var(--ui-danger);
    font-size: var(--size-sm);
  }

  .github-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    padding: 8px 10px;
    background: transparent;
    border: none;
    border-radius: 4px;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .github-item:hover,
  .github-item.selected {
    background-color: var(--bg-hover);
  }

  .github-item-main {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-sm);
    color: var(--text-primary);
  }

  .github-item-number {
    color: var(--text-muted);
    font-family: 'SF Mono', 'Menlo', monospace;
    font-size: var(--size-xs);
    flex-shrink: 0;
  }

  .github-item-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .github-draft-badge {
    flex-shrink: 0;
    padding: 1px 6px;
    background-color: var(--bg-hover);
    border: 1px solid var(--border-muted);
    border-radius: 10px;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .github-item-meta {
    font-size: var(--size-xs);
    color: var(--text-faint);
    padding-left: 0;
  }

  .github-empty {
    padding: 16px;
    text-align: center;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .error-message {
    padding: 10px 12px;
    background-color: var(--ui-danger-bg);
    border-radius: 6px;
    color: var(--ui-danger);
    font-size: var(--size-sm);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 8px;
  }

  .cancel-button {
    padding: 8px 16px;
    background: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: all 0.15s;
  }

  .cancel-button:hover {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
  }

  .create-button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    background-color: var(--ui-accent);
    border: none;
    border-radius: 6px;
    color: var(--bg-deepest);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.15s;
  }

  .create-button:hover:not(:disabled) {
    background-color: var(--ui-accent-hover);
  }

  .create-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
