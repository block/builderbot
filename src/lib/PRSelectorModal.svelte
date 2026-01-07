<script lang="ts">
  import { X, AlertCircle, RefreshCw, GitPullRequest, ExternalLink } from 'lucide-svelte';
  import { checkGitHubAuth, listPullRequests, fetchPRBranch } from './services/git';
  import type { PullRequest, GitHubAuthStatus } from './types';

  interface Props {
    repoPath: string | null;
    onSubmit: (base: string, head: string, label: string) => void;
    onClose: () => void;
  }

  let { repoPath, onSubmit, onClose }: Props = $props();

  // State
  let authStatus = $state<GitHubAuthStatus | null>(null);
  let pullRequests = $state<PullRequest[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let searchQuery = $state('');
  let selectedIndex = $state(0);
  let fetching = $state(false);

  // Filtered PRs based on search
  let filteredPRs = $derived.by(() => {
    if (!searchQuery.trim()) return pullRequests;
    const query = searchQuery.toLowerCase();
    return pullRequests.filter(
      (pr) =>
        pr.title.toLowerCase().includes(query) ||
        pr.number.toString().includes(query) ||
        pr.author.toLowerCase().includes(query)
    );
  });

  // Load auth status and PRs on mount
  $effect(() => {
    loadPRs(false);
  });

  async function loadPRs(forceRefresh: boolean) {
    loading = true;
    error = null;

    try {
      // Check auth first
      authStatus = await checkGitHubAuth();

      if (!authStatus.authenticated) {
        loading = false;
        return;
      }

      // Fetch PRs using the provided repo path
      pullRequests = await listPullRequests(repoPath ?? undefined, forceRefresh);
      selectedIndex = 0;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function handleRefresh() {
    loadPRs(true);
  }

  async function selectPR(pr: PullRequest) {
    // Fetch the PR using GitHub's PR refs (works for both same-repo and fork PRs)
    // This also returns the merge-base SHA to use for the diff
    fetching = true;
    error = null;

    try {
      const mergeBase = await fetchPRBranch(pr.base_ref, pr.number, repoPath ?? undefined);
      // Use merge-base as the base ref so we show only the PR's changes,
      // not changes that happened on the base branch since the PR was created
      // Use the PR ref as head (refs/pull/{number}/head is fetched locally)
      const prRef = `refs/pull/${pr.number}/head`;
      onSubmit(mergeBase, prRef, `PR #${pr.number}`);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      fetching = false;
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      onClose();
      event.preventDefault();
    } else if (event.key === 'Enter' && filteredPRs.length > 0 && !fetching) {
      selectPR(filteredPRs[selectedIndex]);
      event.preventDefault();
    } else if (event.key === 'ArrowDown') {
      event.preventDefault();
      selectedIndex = Math.min(selectedIndex + 1, filteredPRs.length - 1);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      selectedIndex = Math.max(selectedIndex - 1, 0);
    }
  }

  // Reset selection when search changes
  $effect(() => {
    const _ = searchQuery;
    selectedIndex = 0;
  });

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      onClose();
    }
  }

  function formatLineChanges(additions: number, deletions: number): string {
    if (additions === 0 && deletions === 0) return '';
    const parts = [];
    if (additions > 0) parts.push(`+${additions}`);
    if (deletions > 0) parts.push(`-${deletions}`);
    return parts.join(' ');
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
      <h2>Select Pull Request</h2>
      <div class="header-actions">
        {#if authStatus?.authenticated && !loading}
          <button class="icon-btn" onclick={handleRefresh} title="Refresh PR list">
            <RefreshCw size={14} />
          </button>
        {/if}
        <button class="icon-btn" onclick={onClose}>
          <X size={16} />
        </button>
      </div>
    </header>

    <div class="modal-body">
      {#if loading}
        <div class="loading-state">
          <RefreshCw size={24} class="spinner" />
          <span>Loading pull requests...</span>
        </div>
      {:else if fetching}
        <div class="loading-state">
          <RefreshCw size={24} class="spinner" />
          <span>Fetching branch...</span>
        </div>
      {:else if !authStatus?.authenticated}
        <div class="auth-required">
          <AlertCircle size={32} />
          <h3>GitHub CLI Required</h3>
          <p>To view pull requests, you need to authenticate with the GitHub CLI.</p>

          {#if authStatus?.setup_hint}
            <div class="setup-hint">
              <code>{authStatus.setup_hint}</code>
            </div>
          {/if}

          <div class="setup-steps">
            <p><strong>Setup:</strong></p>
            <ol>
              <li>
                Install GitHub CLI: <code>brew install gh</code>
              </li>
              <li>
                Authenticate: <code>gh auth login</code>
              </li>
              <li>Restart Staged and try again</li>
            </ol>
          </div>

          <a
            href="https://cli.github.com/"
            target="_blank"
            rel="noopener noreferrer"
            class="docs-link"
          >
            <ExternalLink size={12} />
            GitHub CLI Documentation
          </a>
        </div>
      {:else if error}
        <div class="error-state">
          <AlertCircle size={24} />
          <span>{error}</span>
          <button class="retry-btn" onclick={() => loadPRs(true)}>Try Again</button>
        </div>
      {:else if pullRequests.length === 0}
        <div class="empty-state">
          <GitPullRequest size={32} />
          <span>No open pull requests</span>
        </div>
      {:else}
        <div class="search-container">
          <!-- svelte-ignore a11y_autofocus -->
          <input
            type="text"
            class="search-input"
            placeholder="Search PRs by title, number, or author..."
            bind:value={searchQuery}
            autofocus
          />
        </div>

        <div class="pr-list">
          {#each filteredPRs as pr, i}
            <button
              class="pr-item"
              class:selected={i === selectedIndex}
              onclick={() => selectPR(pr)}
              onmouseenter={() => (selectedIndex = i)}
            >
              <div class="pr-header">
                <span class="pr-number">#{pr.number}</span>
                <span class="pr-title">{pr.title}</span>
                {#if pr.draft}
                  <span class="draft-badge">Draft</span>
                {/if}
              </div>
              <div class="pr-meta">
                <span class="pr-author">@{pr.author}</span>
                <span class="pr-refs">{pr.base_ref} ← {pr.head_ref}</span>
                {#if pr.additions > 0 || pr.deletions > 0}
                  <span class="pr-changes">{formatLineChanges(pr.additions, pr.deletions)}</span>
                {/if}
              </div>
            </button>
          {/each}

          {#if filteredPRs.length === 0 && searchQuery}
            <div class="no-results">No PRs match "{searchQuery}"</div>
          {/if}
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 520px;
    max-width: 90vw;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
    flex-shrink: 0;
  }

  .modal-header h2 {
    margin: 0;
    font-size: var(--size-base);
    font-weight: 600;
    color: var(--text-primary);
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .icon-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 6px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .icon-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .modal-body {
    padding: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    min-height: 200px;
  }

  /* Loading state */
  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 48px 20px;
    color: var(--text-muted);
  }

  .loading-state :global(.spinner) {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  /* Auth required state */
  .auth-required {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 32px 24px;
    text-align: center;
    color: var(--text-muted);
  }

  .auth-required :global(svg) {
    color: var(--ui-warning);
    margin-bottom: 12px;
  }

  .auth-required h3 {
    margin: 0 0 8px;
    font-size: var(--size-base);
    font-weight: 600;
    color: var(--text-primary);
  }

  .auth-required p {
    margin: 0 0 16px;
    font-size: var(--size-sm);
    line-height: 1.5;
  }

  .setup-hint {
    width: 100%;
    padding: 10px 12px;
    background: var(--bg-primary);
    border-radius: 6px;
    margin-bottom: 16px;
  }

  .setup-hint code {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-xs);
    color: var(--ui-warning);
  }

  .setup-steps {
    text-align: left;
    width: 100%;
    padding: 12px 16px;
    background: var(--bg-primary);
    border-radius: 6px;
    margin-bottom: 16px;
  }

  .setup-steps p {
    margin: 0 0 8px;
    font-size: var(--size-sm);
  }

  .setup-steps ol {
    margin: 0;
    padding-left: 20px;
    font-size: var(--size-sm);
  }

  .setup-steps li {
    margin-bottom: 4px;
  }

  .setup-steps code {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-xs);
    background: var(--bg-hover);
    padding: 2px 6px;
    border-radius: 4px;
  }

  .docs-link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--ui-accent);
    font-size: var(--size-sm);
    text-decoration: none;
  }

  .docs-link:hover {
    text-decoration: underline;
  }

  /* Error state */
  .error-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 48px 20px;
    color: var(--ui-danger);
    text-align: center;
  }

  .retry-btn {
    padding: 8px 16px;
    background: var(--bg-hover);
    border: none;
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .retry-btn:hover {
    background: var(--border-subtle);
  }

  /* Empty state */
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 48px 20px;
    color: var(--text-muted);
  }

  /* Search */
  .search-container {
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-subtle);
    flex-shrink: 0;
  }

  .search-input {
    width: 100%;
    padding: 10px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    box-sizing: border-box;
    transition:
      border-color 0.1s,
      background-color 0.1s;
  }

  .search-input::placeholder {
    color: var(--text-faint);
  }

  .search-input:focus {
    outline: none;
    border-color: var(--border-emphasis);
    background-color: var(--bg-hover);
  }

  /* PR list */
  .pr-list {
    overflow-y: auto;
    flex: 1;
  }

  .pr-item {
    display: flex;
    flex-direction: column;
    gap: 4px;
    width: 100%;
    padding: 12px 16px;
    background: none;
    border: none;
    border-bottom: 1px solid var(--border-subtle);
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .pr-item:last-child {
    border-bottom: none;
  }

  .pr-item:hover,
  .pr-item.selected {
    background-color: var(--bg-hover);
  }

  .pr-header {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .pr-number {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--ui-accent);
  }

  .pr-title {
    flex: 1;
    font-size: var(--size-sm);
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .draft-badge {
    padding: 2px 6px;
    background: var(--bg-primary);
    border-radius: 4px;
    font-size: var(--size-xs);
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .pr-meta {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .pr-author {
    color: var(--text-faint);
  }

  .pr-refs {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
  }

  .pr-changes {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    color: var(--text-faint);
  }

  .no-results {
    padding: 24px 16px;
    text-align: center;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }
</style>
