<!--
  HomePage — Diff mode picker and repo info.

  Shows cards for each diff mode with file counts.
  Clicking a card navigates to the diff view.
-->
<script lang="ts">
  import { GitBranch, FolderOpen, ChevronDown, FileEdit, GitCommitHorizontal } from 'lucide-svelte';
  import * as commands from './commands';
  import type { RepoInfo, CommitInfo, DiffSpec } from './commands';

  // ==========================================================================
  // Props
  // ==========================================================================

  interface Props {
    onOpenDiff: (spec: DiffSpec, label: string) => void;
  }

  let { onOpenDiff }: Props = $props();

  // ==========================================================================
  // State
  // ==========================================================================

  let repoInfo = $state<RepoInfo | null>(null);
  let commits = $state<CommitInfo[]>([]);
  let loadingRepo = $state(true);
  let repoError = $state<string | null>(null);

  // File counts per mode
  let allCount = $state<number | null>(null);
  let branchCount = $state<number | null>(null);

  // Commit picker
  let showCommitPicker = $state(false);
  let loadingCommits = $state(false);

  // ==========================================================================
  // Load data
  // ==========================================================================

  async function loadRepoInfo() {
    loadingRepo = true;
    repoError = null;
    try {
      repoInfo = await commands.getRepoInfo();
      // Load file counts in parallel
      loadFileCounts();
    } catch (e) {
      repoError = e instanceof Error ? e.message : String(e);
    } finally {
      loadingRepo = false;
    }
  }

  async function loadFileCounts() {
    // All changes
    commands
      .listDiffFiles(commands.specUncommitted())
      .then((r) => (allCount = r.files.length))
      .catch(() => (allCount = null));

    // Branch
    commands
      .listDiffFiles(commands.specBranch())
      .then((r) => (branchCount = r.files.length))
      .catch(() => (branchCount = null));
  }

  async function loadCommits() {
    if (commits.length > 0) return;
    loadingCommits = true;
    try {
      commits = await commands.listRecentCommits(20);
    } catch (e) {
      console.error('Failed to load commits:', e);
    } finally {
      loadingCommits = false;
    }
  }

  async function handleOpenRepo() {
    try {
      const result = await commands.openRepoDialog();
      if (result) {
        // Reset all state and reload
        repoInfo = null;
        commits = [];
        allCount = null;
        branchCount = null;
        showCommitPicker = false;
        await loadRepoInfo();
      }
    } catch (e) {
      console.error('Failed to open repo:', e);
    }
  }

  // Init
  loadRepoInfo();

  // ==========================================================================
  // Card handlers
  // ==========================================================================

  function openAllChanges() {
    onOpenDiff(commands.specUncommitted(), 'All Changes');
  }

  function openBranch() {
    if (!repoInfo) return;
    const defaultBranch = repoInfo.defaultBranch.replace('origin/', '');
    onOpenDiff(commands.specBranch(), `Branch vs ${defaultBranch}`);
  }

  function openCommit(commit: CommitInfo) {
    onOpenDiff(commands.specCommit(commit.sha), `Commit ${commit.shortSha}`);
  }

  function toggleCommitPicker() {
    showCommitPicker = !showCommitPicker;
    if (showCommitPicker) {
      loadCommits();
    }
  }

  // ==========================================================================
  // Helpers
  // ==========================================================================

  function shortenPath(path: string): string {
    const home = path.replace(/^\/Users\/[^/]+/, '~');
    const parts = home.split('/');
    if (parts.length <= 3) return home;
    return parts.slice(0, 1).concat('...', parts.slice(-2)).join('/');
  }

  function timeAgo(timestamp: number): string {
    const now = Date.now() / 1000;
    const diff = now - timestamp;
    if (diff < 60) return 'just now';
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    return `${Math.floor(diff / 86400)}d ago`;
  }
</script>

<div class="home-page" data-tauri-drag-region>
  <div class="home-content">
    <!-- Repo info -->
    {#if loadingRepo}
      <div class="loading">
        <span class="spinner"></span>
        <span>Loading repository...</span>
      </div>
    {:else if repoError}
      <div class="error-box">
        <p>{repoError}</p>
        <button class="open-btn" onclick={handleOpenRepo}>Open Repository...</button>
      </div>
    {:else if repoInfo}
      <div class="repo-bar">
        <span class="repo-path" title={repoInfo.path}>{shortenPath(repoInfo.path)}</span>
        <span class="separator">&#183;</span>
        <span class="branch-badge">
          <GitBranch size={12} />
          {repoInfo.branch}
        </span>
        {#if repoInfo.commitsAhead > 0}
          <span class="separator">&#183;</span>
          <span class="ahead-badge"
            >{repoInfo.commitsAhead} ahead of {repoInfo.defaultBranch.replace('origin/', '')}</span
          >
        {/if}
        <button class="open-link" onclick={handleOpenRepo} title="Open a different repository">
          <FolderOpen size={14} />
        </button>
      </div>

      <!-- Mode picker cards -->
      <div class="mode-cards">
        <button class="mode-card" onclick={openAllChanges}>
          <div class="card-icon"><FileEdit size={20} /></div>
          <div class="card-label">All Changes</div>
          <div class="card-desc">Working tree vs HEAD</div>
          {#if allCount !== null}
            <div class="card-count">{allCount} file{allCount === 1 ? '' : 's'}</div>
          {/if}
        </button>

        <button class="mode-card" onclick={openBranch}>
          <div class="card-icon"><GitBranch size={20} /></div>
          <div class="card-label">Full Branch</div>
          <div class="card-desc">vs {repoInfo.defaultBranch.replace('origin/', '')}</div>
          {#if branchCount !== null}
            <div class="card-count">{branchCount} file{branchCount === 1 ? '' : 's'}</div>
          {/if}
        </button>

        <button class="mode-card" onclick={toggleCommitPicker}>
          <div class="card-icon"><GitCommitHorizontal size={20} /></div>
          <div class="card-label">Commit</div>
          <div class="card-desc">Pick a commit</div>
          <div class="card-chevron" class:open={showCommitPicker}>
            <ChevronDown size={14} />
          </div>
        </button>
      </div>

      <!-- Commit picker -->
      {#if showCommitPicker}
        <div class="commit-picker">
          {#if loadingCommits}
            <div class="commit-loading">
              <span class="spinner small"></span>
              <span>Loading commits...</span>
            </div>
          {:else if commits.length === 0}
            <div class="commit-empty">No commits found</div>
          {:else}
            {#each commits as commit (commit.sha)}
              <button class="commit-row" onclick={() => openCommit(commit)}>
                <span class="commit-sha">{commit.shortSha}</span>
                <span class="commit-message">{commit.message}</span>
                <span class="commit-time">{timeAgo(commit.timestamp)}</span>
              </button>
            {/each}
          {/if}
        </div>
      {/if}
    {/if}
  </div>
</div>

<style>
  .home-page {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 100vh;
    padding: 60px 32px 32px;
    -webkit-app-region: drag;
  }

  .home-content {
    width: 100%;
    max-width: 640px;
    -webkit-app-region: no-drag;
  }

  .loading {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    color: var(--text-muted);
  }

  .error-box {
    text-align: center;
    color: var(--text-muted);
  }

  .error-box p {
    margin-bottom: 16px;
  }

  /* Repo bar */
  .repo-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    margin-bottom: 24px;
    border-radius: 8px;
    background-color: var(--bg-chrome);
    font-size: var(--size-sm);
    color: var(--text-muted);
    flex-wrap: wrap;
  }

  .repo-path {
    font-family: 'SF Mono', 'Menlo', 'Monaco', monospace;
    font-size: calc(var(--size-xs));
    color: var(--text-primary);
  }

  .separator {
    color: var(--text-faint);
  }

  .branch-badge {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-accent);
  }

  .ahead-badge {
    font-size: var(--size-xs);
    color: var(--status-added);
  }

  .open-link {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    margin-left: auto;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .open-link:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  /* Mode cards */
  .mode-cards {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 12px;
    margin-bottom: 16px;
  }

  .mode-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 20px 16px;
    background-color: var(--bg-chrome);
    border: 1px solid var(--border-subtle);
    border-radius: 12px;
    cursor: pointer;
    color: var(--text-primary);
    font-family: inherit;
    transition:
      border-color 0.15s,
      background-color 0.15s,
      box-shadow 0.15s;
  }

  .mode-card:hover {
    border-color: var(--border-muted);
    background-color: var(--bg-elevated);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  }

  .card-icon {
    color: var(--text-accent);
    margin-bottom: 4px;
  }

  .card-label {
    font-weight: 600;
    font-size: var(--size-md);
  }

  .card-desc {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .card-count {
    font-size: var(--size-xs);
    color: var(--text-faint);
    padding: 2px 8px;
    border-radius: 10px;
    background-color: var(--bg-hover);
    margin-top: 4px;
  }

  .card-chevron {
    color: var(--text-faint);
    transition: transform 0.15s;
  }

  .card-chevron.open {
    transform: rotate(180deg);
  }

  /* Commit picker */
  .commit-picker {
    background-color: var(--bg-chrome);
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    max-height: 320px;
    overflow-y: auto;
  }

  .commit-loading,
  .commit-empty {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 16px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .commit-row {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 8px 12px;
    background: none;
    border: none;
    border-bottom: 1px solid var(--border-subtle);
    color: var(--text-primary);
    font-family: inherit;
    font-size: var(--size-sm);
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .commit-row:last-child {
    border-bottom: none;
  }

  .commit-row:hover {
    background-color: var(--bg-hover);
  }

  .commit-sha {
    flex-shrink: 0;
    font-family: 'SF Mono', 'Menlo', 'Monaco', monospace;
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-accent);
    min-width: 60px;
  }

  .commit-message {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .commit-time {
    flex-shrink: 0;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  /* Spinner */
  .spinner {
    display: inline-block;
    width: 16px;
    height: 16px;
    border: 2px solid var(--border-muted);
    border-top-color: var(--text-accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  .spinner.small {
    width: 14px;
    height: 14px;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .open-btn {
    padding: 8px 16px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    color: var(--text-primary);
    font-family: inherit;
    font-size: var(--size-md);
    cursor: pointer;
    transition:
      background-color 0.1s,
      border-color 0.1s;
  }

  .open-btn:hover {
    background-color: var(--bg-hover);
    border-color: var(--border-emphasis);
  }
</style>
