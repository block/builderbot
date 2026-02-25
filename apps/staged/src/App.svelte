<!--
  App.svelte — Single-page diff viewer for Staged.

  Opens directly to the diff view with all controls in the titlebar:
  - Left: traffic-light padding (macOS close/min/max buttons)
  - Center: diff mode selector (All Changes / Branch / Commit dropdown)
  - Right: theme picker, repo name + folder picker

  No separate home page — the diff selection lives in the top bar.
-->
<script lang="ts">
  import {
    GitBranch,
    FolderOpen,
    ChevronDown,
    FileEdit,
    GitCommitHorizontal,
    Palette,
    ChevronRight,
    Folder,
    CirclePlus,
    CircleMinus,
    CircleArrowUp,
    MessageSquare,
    Copy,
    Check,
    Trash2,
    ListChecks,
  } from 'lucide-svelte';
  import {
    DiffViewer,
    CrossFileSearchBar,
    FileSearchResults,
  } from '@builderbot/diff-viewer/components';
  import {
    buildFileEntries,
    buildTree,
    compactTree,
    formatLineRange,
    truncateText,
    type FileEntry,
    type TreeNode,
    setupDiffKeyboardNav,
    getMatchSnippet,
    getTextLines,
    type SearchMatch,
  } from '@builderbot/diff-viewer/utils';
  import { createSearchState } from '@builderbot/diff-viewer/state';
  import type { FileDiff, FileDiffSummary, Comment, Span } from '@builderbot/diff-viewer/types';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import * as commands from './lib/commands';
  import type { DiffSpec, RepoInfo, CommitInfo, RecentRepo } from './lib/commands';
  import FolderPickerModal from './lib/FolderPickerModal.svelte';
  import ThemePicker from './lib/ThemePicker.svelte';
  import GitTreeAnimation from './lib/GitTreeAnimation.svelte';
  import MarkIcon from './lib/MarkIcon.svelte';
  import { initPreferences } from './lib/preferences.svelte';

  // ==========================================================================
  // State: App initialization
  // ==========================================================================

  let initialized = $state(false);

  // ==========================================================================
  // State: Repo info
  // ==========================================================================

  let repoInfo = $state<RepoInfo | null>(null);
  let repoError = $state<string | null>(null);

  // ==========================================================================
  // State: Diff mode
  // ==========================================================================

  type DiffMode = 'all' | 'staged' | 'branch' | 'commit';

  let diffMode = $state<DiffMode>('all');
  let diffSpec = $state<DiffSpec>(commands.specUncommitted());
  let diffLabel = $state('All Changes');

  // Commit picker
  let commits = $state<CommitInfo[]>([]);
  let showCommitPicker = $state(false);
  let loadingCommits = $state(false);
  let selectedCommit = $state<CommitInfo | null>(null);

  // ==========================================================================
  // State: Diff viewer
  // ==========================================================================

  let files = $state<FileDiffSummary[]>([]);
  let diffCache = $state(new Map<string, FileDiff>());
  let selectedFile = $state<string | null>(null);
  let loading = $state(true);
  let loadingFile = $state<string | null>(null);
  let error = $state<string | null>(null);

  let localComments = $state<Comment[]>([]);
  let copiedFeedback = $state(false);

  let collapsedDirs = $state(new Set<string>());

  let selectionGeneration = 0;

  // ==========================================================================
  // State: Search
  // ==========================================================================

  // svelte-ignore state_referenced_locally
  const searchState = createSearchState();

  // ==========================================================================
  // State: Modals
  // ==========================================================================

  let showFolderPicker = $state(false);
  let showThemePicker = $state(false);
  let suggestedRepos = $state<RecentRepo[]>([]);

  // ==========================================================================
  // Derived
  // ==========================================================================

  let currentDiff = $derived(selectedFile ? (diffCache.get(selectedFile) ?? null) : null);
  let fileEntries = $derived(buildFileEntries(files, [], localComments));
  let fileTree = $derived(compactTree(buildTree(fileEntries)));

  let repoName = $derived(repoInfo ? repoInfo.path.split('/').pop() || repoInfo.path : '');

  // ==========================================================================
  // Initialization
  // ==========================================================================

  async function init() {
    try {
      await initPreferences();

      // Pre-load suggested repos in background
      commands
        .findRecentRepos(24, 10)
        .then((repos) => {
          suggestedRepos = repos;
        })
        .catch(() => {});

      // Check CLI launch args
      const args = await commands.getLaunchArgs();
      if (args.mode) {
        switch (args.mode) {
          case 'all':
            setMode('all');
            break;
          case 'staged':
            setMode('staged');
            break;
          case 'branch':
            setMode('branch');
            break;
          case 'commit':
            if (args.commit) {
              selectCommitBySha(args.commit);
            } else {
              selectCommitBySha('HEAD');
            }
            break;
          default:
            setMode('all');
        }
      }

      await loadRepoInfo();
    } catch (e) {
      console.error('Failed to initialize:', e);
    } finally {
      initialized = true;
    }
  }

  init();

  // ==========================================================================
  // Repo info
  // ==========================================================================

  async function loadRepoInfo() {
    repoError = null;
    try {
      repoInfo = await commands.getRepoInfo();
      loadDiff();
    } catch (e) {
      repoError = e instanceof Error ? e.message : String(e);
      loading = false;
    }
  }

  // ==========================================================================
  // Diff mode switching
  // ==========================================================================

  function setMode(mode: DiffMode, commit?: CommitInfo) {
    showCommitPicker = false;
    diffMode = mode;

    switch (mode) {
      case 'all':
        diffSpec = commands.specUncommitted();
        diffLabel = 'All Changes';
        selectedCommit = null;
        break;
      case 'staged':
        diffSpec = commands.specStaged();
        diffLabel = 'Staged';
        selectedCommit = null;
        break;
      case 'branch':
        diffSpec = commands.specBranch();
        diffLabel = repoInfo
          ? `Branch vs ${repoInfo.defaultBranch.replace('origin/', '')}`
          : 'Full Branch';
        selectedCommit = null;
        break;
      case 'commit':
        if (commit) {
          diffSpec = commands.specCommit(commit.sha);
          diffLabel = `Commit ${commit.shortSha}`;
          selectedCommit = commit;
        }
        break;
    }

    files = [];
    diffCache = new Map();
    selectedFile = null;
    localComments = [];
    error = null;
    loadDiff();
  }

  function selectCommitBySha(sha: string) {
    diffMode = 'commit';
    diffSpec = commands.specCommit(sha);
    diffLabel = `Commit ${sha.slice(0, 7)}`;
    selectedCommit = null;

    files = [];
    diffCache = new Map();
    selectedFile = null;
    localComments = [];
    error = null;
    loadDiff();
  }

  async function toggleCommitPicker() {
    showCommitPicker = !showCommitPicker;
    if (showCommitPicker && commits.length === 0) {
      loadingCommits = true;
      try {
        commits = await commands.listRecentCommits(20);
      } catch (e) {
        console.error('Failed to load commits:', e);
      } finally {
        loadingCommits = false;
      }
    }
  }

  // ==========================================================================
  // Load diff
  // ==========================================================================

  async function loadDiff() {
    loading = true;
    error = null;
    try {
      const response = await commands.listDiffFiles(diffSpec);
      files = response.files;
      if (files.length > 0) {
        const firstPath = files[0].after ?? files[0].before ?? '';
        await selectFile(firstPath);
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      files = [];
    } finally {
      loading = false;
    }
  }

  async function selectFile(path: string | null) {
    const thisGeneration = ++selectionGeneration;
    selectedFile = path;

    if (path && !diffCache.has(path)) {
      loadingFile = path;
      try {
        const diff = await commands.getFileDiff(diffSpec, path);
        if (selectionGeneration !== thisGeneration) return;
        const newCache = new Map(diffCache);
        newCache.set(path, diff);
        diffCache = newCache;
      } catch (e) {
        console.error(`Failed to load diff for ${path}:`, e);
      } finally {
        loadingFile = null;
      }
    }
  }

  // ==========================================================================
  // Comment handling
  // ==========================================================================

  let nextCommentId = 0;

  async function handleAddComment(path: string, span: Span, content: string): Promise<void> {
    const comment: Comment = {
      id: `local-${++nextCommentId}`,
      path,
      span,
      content,
      author: 'user',
      commentType: null,
      createdAt: Date.now(),
    };
    localComments = [...localComments, comment];
  }

  async function handleUpdateComment(commentId: string, content: string): Promise<void> {
    localComments = localComments.map((c) => (c.id === commentId ? { ...c, content } : c));
  }

  async function handleDeleteComment(commentId: string): Promise<void> {
    localComments = localComments.filter((c) => c.id !== commentId);
  }

  async function handleCopyComments() {
    if (!localComments.length) return;
    const lines: string[] = [];
    for (const c of localComments) {
      lines.push(`**${c.path}** ${formatLineRange(c.span)}`);
      lines.push(c.content);
      lines.push('');
    }
    try {
      await navigator.clipboard.writeText(lines.join('\n'));
      copiedFeedback = true;
      setTimeout(() => (copiedFeedback = false), 1500);
    } catch (e) {
      console.error('Failed to copy:', e);
    }
  }

  // ==========================================================================
  // Sidebar helpers
  // ==========================================================================

  function handleSelectFile(file: FileEntry) {
    selectFile(file.path);
  }

  // Wrapper for search that returns the loaded diff
  async function loadFileDiffForSearch(path: string) {
    await selectFile(path);
    return currentDiff;
  }

  function toggleDir(path: string) {
    const newSet = new Set(collapsedDirs);
    if (newSet.has(path)) newSet.delete(path);
    else newSet.add(path);
    collapsedDirs = newSet;
  }

  function isCollapsed(path: string): boolean {
    return collapsedDirs.has(path);
  }

  // ==========================================================================
  // Folder picker
  // ==========================================================================

  async function handleFolderSelect(path: string) {
    showFolderPicker = false;
    try {
      await commands.setRepoPath(path);
      repoInfo = null;
      commits = [];
      showCommitPicker = false;
      selectedCommit = null;
      diffMode = 'all';
      diffSpec = commands.specUncommitted();
      diffLabel = 'All Changes';
      files = [];
      diffCache = new Map();
      selectedFile = null;
      localComments = [];
      error = null;
      await loadRepoInfo();
    } catch (e) {
      console.error('Failed to open repo:', e);
    }
  }

  // ==========================================================================
  // Helpers
  // ==========================================================================

  function startDrag(e: PointerEvent) {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    const isInteractive = target.closest('button, a, input, [role="button"]');
    if (!isInteractive) {
      e.preventDefault();
      getCurrentWindow().startDragging();
    }
  }

  function timeAgo(timestamp: number): string {
    const now = Date.now() / 1000;
    const diff = now - timestamp;
    if (diff < 60) return 'just now';
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    return `${Math.floor(diff / 86400)}d ago`;
  }

  // ==========================================================================
  // Search helpers
  // ==========================================================================

  // Helper to get snippet for a search result
  function getSnippet(match: SearchMatch, filePath: string): string {
    const diff = diffCache.get(filePath);
    if (!diff) return '';
    const afterLines = getTextLines(diff, 'after');
    return getMatchSnippet(match, afterLines);
  }

  // Handle clicking a search result
  async function handleSearchResultClick(
    filePath: string,
    match: SearchMatch,
    globalIndex: number
  ) {
    // Update current result index
    searchState.setCurrentResult(globalIndex);

    // Auto-expand search results for this file
    if (searchState.areSearchResultsCollapsed(filePath)) {
      searchState.toggleSearchResults(filePath);
    }

    // Select the file and scroll to the match
    await selectFile(filePath);
    // Note: Scrolling to the specific line would require additional integration
  }

  // Initialize collapsed state when search results are ready
  $effect(() => {
    if (
      searchState.state.isOpen &&
      searchState.state.fileResults.size > 0 &&
      !searchState.state.loading
    ) {
      searchState.initializeCollapsedState(files);
    }
  });

  // ==========================================================================
  // Keyboard shortcuts
  // ==========================================================================

  // Set up keyboard navigation for diff viewer and search
  $effect(() => {
    const cleanup = setupDiffKeyboardNav({
      onOpenSearch: () => searchState.openSearch(),
      onNextSearchResult: async () => {
        const result = await searchState.goToNextResult(files);
        if (result) {
          // Auto-expand search results for this file
          if (searchState.areSearchResultsCollapsed(result.filePath)) {
            searchState.toggleSearchResults(result.filePath);
          }
          await selectFile(result.filePath);
          // TODO: Scroll to the specific line (result.match.lineIndex)
        }
      },
      onPrevSearchResult: async () => {
        const result = await searchState.goToPrevResult(files);
        if (result) {
          // Auto-expand search results for this file
          if (searchState.areSearchResultsCollapsed(result.filePath)) {
            searchState.toggleSearchResults(result.filePath);
          }
          await selectFile(result.filePath);
          // TODO: Scroll to the specific line (result.match.lineIndex)
        }
      },
    });

    return cleanup;
  });
</script>

{#if initialized}
  <div class="app">
    <!-- Titlebar -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="titlebar" onpointerdown={startDrag}>
      <div class="titlebar-left"></div>

      <div class="titlebar-center">
        <div class="mode-segmented">
          <button
            class="mode-seg"
            class:active={diffMode === 'all'}
            onclick={() => setMode('all')}
            title="All uncommitted changes"
          >
            <FileEdit size={12} />
            <span>All</span>
          </button>
          <button
            class="mode-seg"
            class:active={diffMode === 'staged'}
            onclick={() => setMode('staged')}
            title="Staged changes only"
          >
            <ListChecks size={12} />
            <span>Staged</span>
          </button>
          <button
            class="mode-seg"
            class:active={diffMode === 'branch'}
            onclick={() => setMode('branch')}
            title="Full branch diff"
          >
            <GitBranch size={12} />
            <span>Branch</span>
          </button>

          <div class="commit-picker-wrap">
            <button
              class="mode-seg last"
              class:active={diffMode === 'commit'}
              onclick={toggleCommitPicker}
              title="Pick a commit"
            >
              <GitCommitHorizontal size={12} />
              <span>{selectedCommit ? selectedCommit.shortSha : 'Commit'}</span>
              <ChevronDown size={11} class="chevron" />
            </button>

            {#if showCommitPicker}
              <div class="commit-dropdown">
                {#if loadingCommits}
                  <div class="commit-loading">
                    <span class="spinner small"></span>
                    Loading commits...
                  </div>
                {:else if commits.length === 0}
                  <div class="commit-empty">No commits found</div>
                {:else}
                  {#each commits as commit (commit.sha)}
                    <button
                      class="commit-row"
                      class:active={selectedCommit?.sha === commit.sha}
                      onclick={() => setMode('commit', commit)}
                    >
                      <span class="commit-sha">{commit.shortSha}</span>
                      <span class="commit-message">{commit.message}</span>
                      <span class="commit-time">{timeAgo(commit.timestamp)}</span>
                    </button>
                  {/each}
                {/if}
              </div>
            {/if}
          </div>
        </div>

        {#if files.length > 0}
          <span class="file-count">{files.length} file{files.length === 1 ? '' : 's'}</span>
        {/if}
      </div>

      <div class="titlebar-right">
        {#if repoInfo}
          <span class="repo-info">
            <GitBranch size={12} />
            <span class="branch-name">{repoInfo.branch}</span>
          </span>
          <button
            class="repo-btn"
            onclick={() => (showFolderPicker = true)}
            title="Switch repository"
          >
            <FolderOpen size={13} />
            <span>{repoName}</span>
          </button>
        {/if}
        <button
          class="theme-btn"
          onclick={() => (showThemePicker = !showThemePicker)}
          title="Change theme"
        >
          <Palette size={14} />
        </button>
      </div>
    </div>

    <!-- Body -->
    <div class="body">
      <div class="diff-viewer-container">
        {#if loading}
          <div class="center-message">
            <span class="spinner"></span>
            <span>Loading diff...</span>
          </div>
        {:else if error}
          <div class="center-message error">
            <span>{error}</span>
          </div>
        {:else if repoError}
          <div class="center-message error">
            <span>{repoError}</span>
            <button class="open-btn" onclick={() => (showFolderPicker = true)}>
              Open Repository...
            </button>
          </div>
        {:else if files.length === 0}
          <div class="empty-state">
            <div class="empty-content">
              <div class="empty-icon">
                <MarkIcon size={52} />
              </div>
              <h2 class="empty-heading">No changes found</h2>
              <p class="empty-hint">Choose a change set from the toolbar above</p>
            </div>
            <div class="empty-tree">
              <GitTreeAnimation />
            </div>
          </div>
        {:else}
          <DiffViewer
            diff={currentDiff}
            comments={localComments.filter((c) => c.path === selectedFile)}
            loading={loadingFile !== null}
            beforeLabel="before"
            afterLabel="after"
            onAddComment={handleAddComment}
            onUpdateComment={handleUpdateComment}
            onDeleteComment={handleDeleteComment}
          />
        {/if}
      </div>

      {#if files.length > 0}
        <div class="file-sidebar">
          <div class="sidebar-content">
            <!-- Search bar -->
            <CrossFileSearchBar {files} loadFileDiff={loadFileDiffForSearch} {searchState} />

            <div class="section-header">
              <div class="section-left"></div>
              <div class="section-divider">
                <span class="divider-label">CHANGED</span>
                <span class="count-capsule">{fileEntries.length}</span>
              </div>
              <div class="section-right"></div>
            </div>
            <ul class="tree-section">
              {#snippet treeNodes(nodes: TreeNode[], depth: number)}
                {#each nodes as node (node.path)}
                  {#if node.isDir}
                    <li class="tree-item-wrapper">
                      <button
                        class="tree-item dir-item"
                        style="padding-left: {8 + depth * 12}px"
                        onclick={() => toggleDir(node.path)}
                      >
                        <span class="dir-chevron">
                          {#if isCollapsed(node.path)}
                            <ChevronRight size={14} />
                          {:else}
                            <ChevronDown size={14} />
                          {/if}
                        </span>
                        <span class="dir-icon"><Folder size={14} /></span>
                        <span class="dir-name">{node.name}</span>
                      </button>
                      {#if !isCollapsed(node.path)}
                        <ul class="tree-children">
                          {@render treeNodes(node.children, depth + 1)}
                        </ul>
                      {/if}
                    </li>
                  {:else if node.file}
                    <li class="tree-item-wrapper">
                      <button
                        class="tree-item file-item"
                        class:selected={selectedFile === node.file.path}
                        class:has-search-results={searchState.state.isOpen &&
                          searchState.state.fileResults.has(node.file.path)}
                        style="padding-left: {8 + depth * 12}px"
                        onclick={() => handleSelectFile(node.file!)}
                      >
                        {#if searchState.state.isOpen}
                          {#if searchState.state.fileResults.has(node.file.path)}
                            <span
                              class="search-chevron"
                              onclick={(e) => {
                                e.stopPropagation();
                                searchState.toggleSearchResults(node.file!.path);
                              }}
                              role="button"
                              tabindex="0"
                              onkeydown={(e) => {
                                if (e.key === 'Enter' || e.key === ' ') {
                                  e.preventDefault();
                                  e.stopPropagation();
                                  searchState.toggleSearchResults(node.file!.path);
                                }
                              }}
                            >
                              {#if searchState.areSearchResultsCollapsed(node.file.path)}
                                <ChevronRight size={14} />
                              {:else}
                                <ChevronDown size={14} />
                              {/if}
                            </span>
                          {:else}
                            <span class="search-spacer"></span>
                          {/if}
                        {/if}
                        <span class="status-icon">
                          {#if node.file.status === 'added'}
                            <CirclePlus size={16} />
                          {:else if node.file.status === 'deleted'}
                            <CircleMinus size={16} />
                          {:else}
                            <CircleArrowUp size={16} />
                          {/if}
                        </span>
                        <span class="file-name">{node.name}</span>
                        {#if node.file.commentCount > 0}
                          <span class="comment-indicator">
                            <MessageSquare size={12} />
                          </span>
                        {/if}
                        {#if searchState.state.isOpen && searchState.state.fileResults.has(node.file.path)}
                          {@const resultCount =
                            searchState.state.fileResults.get(node.file.path)?.matches.length ?? 0}
                          <span
                            class="search-result-count"
                            title="{resultCount} search result{resultCount !== 1 ? 's' : ''}"
                          >
                            {resultCount}
                          </span>
                        {/if}
                      </button>

                      <!-- Search results (if search is active and this file has matches) -->
                      {#if searchState.state.isOpen && searchState.state.fileResults.has(node.file.path) && !searchState.areSearchResultsCollapsed(node.file.path)}
                        {@const fileResult = searchState.state.fileResults.get(node.file.path)}
                        {#if fileResult}
                          <FileSearchResults
                            {fileResult}
                            filePath={node.file.path}
                            {depth}
                            {getSnippet}
                            isCurrentResult={(fp, idx) =>
                              searchState.isCurrentResult(files, fp, idx)}
                            onResultClick={handleSearchResultClick}
                            getGlobalIndex={(fp, idx) => searchState.getGlobalIndex(files, fp, idx)}
                            onExpandResults={(fp) => searchState.expandFileResults(fp)}
                            onCollapseResults={(fp) => searchState.collapseFileResults(fp)}
                          />
                        {/if}
                      {/if}
                    </li>
                  {/if}
                {/each}
              {/snippet}
              {@render treeNodes(fileTree, 0)}
            </ul>

            {#if localComments.length > 0}
              <div class="section-header">
                <div class="section-left"></div>
                <div class="section-divider">
                  <span class="divider-label">COMMENTS</span>
                  <span class="count-capsule">{localComments.length}</span>
                </div>
                <div class="section-right">
                  <button
                    class="copy-btn"
                    class:copied={copiedFeedback}
                    onclick={handleCopyComments}
                    title="Copy all comments"
                  >
                    {#if copiedFeedback}
                      <Check size={12} />
                    {:else}
                      <Copy size={12} />
                    {/if}
                  </button>
                </div>
              </div>
              <ul class="tree-section comments-section">
                {#each localComments as comment (comment.id)}
                  <li class="tree-item-wrapper">
                    <div class="comment-item-container">
                      <button
                        class="tree-item comment-item"
                        onclick={() => selectFile(comment.path)}
                      >
                        <span class="comment-icon">
                          <MessageSquare size={12} />
                        </span>
                        <span class="comment-details">
                          <span class="comment-location">
                            <span class="comment-file"
                              >{comment.path.split('/').pop() || comment.path}</span
                            >
                            <span class="comment-line">{formatLineRange(comment.span)}</span>
                          </span>
                          <span class="comment-preview">{truncateText(comment.content)}</span>
                        </span>
                      </button>
                      <button
                        class="comment-delete-btn"
                        onclick={(e) => {
                          e.stopPropagation();
                          handleDeleteComment(comment.id);
                        }}
                        title="Delete comment"
                      >
                        <Trash2 size={12} />
                      </button>
                    </div>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        </div>
      {/if}
    </div>
  </div>

  <!-- Modals -->
  {#if showFolderPicker}
    <FolderPickerModal
      {suggestedRepos}
      currentPath={repoInfo?.path}
      onSelect={handleFolderSelect}
      onClose={() => (showFolderPicker = false)}
    />
  {/if}

  {#if showThemePicker}
    <ThemePicker onClose={() => (showThemePicker = false)} />
  {/if}
{/if}

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background-color: var(--bg-chrome);
  }

  /* Titlebar */
  .titlebar {
    display: flex;
    align-items: center;
    height: 40px;
    flex-shrink: 0;
    border-bottom: 1px solid var(--border-subtle);
    padding: 0 8px;
    gap: 8px;
  }

  .titlebar-left {
    flex: 1 1 0;
    min-width: 72px;
  }

  .titlebar-center {
    position: relative;
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
  }

  .titlebar-right {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
    flex: 1 1 0;
    min-width: 0;
  }

  /* Segmented control */
  .mode-segmented {
    display: flex;
    align-items: center;
    background-color: var(--bg-deepest);
    border: 1px solid var(--border-subtle);
    border-radius: 7px;
    padding: 2px;
    gap: 1px;
  }

  .mode-seg {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 3px 10px;
    background: none;
    border: 1px solid transparent;
    border-radius: 5px;
    color: var(--text-faint);
    cursor: pointer;
    font-size: var(--size-xs);
    font-family: inherit;
    transition:
      color 0.15s,
      background-color 0.15s,
      border-color 0.15s,
      box-shadow 0.15s;
    white-space: nowrap;
  }

  .mode-seg:hover {
    color: var(--text-muted);
  }

  .mode-seg.active {
    color: var(--text-primary);
    background-color: var(--bg-elevated);
    border-color: var(--border-muted);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
  }

  .mode-seg :global(.chevron) {
    color: var(--text-faint);
    margin-left: -2px;
  }

  .file-count {
    position: absolute;
    left: calc(100% + 8px);
    top: 50%;
    transform: translateY(-50%);
    color: var(--text-faint);
    font-size: var(--size-xs);
    white-space: nowrap;
  }

  /* Commit picker */
  .commit-picker-wrap {
    position: relative;
    display: flex;
  }

  .commit-dropdown {
    position: absolute;
    top: calc(100% + 4px);
    left: 50%;
    transform: translateX(-50%);
    width: 400px;
    max-height: 320px;
    overflow-y: auto;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    z-index: 100;
  }

  .commit-loading,
  .commit-empty {
    display: flex;
    align-items: center;
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

  .commit-row.active {
    background-color: var(--ui-selection);
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

  /* Repo info */
  .repo-info {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .branch-name {
    color: var(--text-accent);
  }

  .repo-btn {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 4px 8px;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    font-size: var(--size-xs);
    font-family: inherit;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .repo-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .theme-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .theme-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  /* Body */
  .body {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .diff-viewer-container {
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }

  .center-message {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    height: 100%;
    color: var(--text-muted);
    font-size: var(--size-md);
  }

  .center-message.error {
    color: var(--ui-danger);
  }

  /* Empty state */
  .empty-state {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    height: 100%;
    overflow: hidden;
    animation: empty-fade-in 0.4s ease both;
  }

  @keyframes empty-fade-in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .empty-content {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 16px;
    z-index: 1;
  }

  .empty-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 104px;
    height: 104px;
    border-radius: 26px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    box-shadow:
      0 8px 32px rgba(0, 0, 0, 0.2),
      0 0 0 1px var(--border-subtle);
  }

  .empty-heading {
    font-size: 16px;
    font-weight: 500;
    color: var(--text-primary);
    margin: 0;
    letter-spacing: -0.01em;
  }

  .empty-hint {
    font-size: var(--size-sm);
    color: var(--text-faint);
    margin: 0;
  }

  .empty-tree {
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    opacity: 0.2;
    pointer-events: none;
  }

  .empty-tree :global(.animation-wrapper) {
    width: 100%;
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

  /* File sidebar */
  .file-sidebar {
    width: 240px;
    flex-shrink: 0;
    overflow-y: auto;
    overflow-x: hidden;
  }

  .sidebar-content {
    display: flex;
    flex-direction: column;
    padding: 0;
  }

  .section-header {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    margin: 16px 12px 8px;
    gap: 6px;
  }

  .section-left {
    display: flex;
    align-items: center;
  }

  .section-left::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--border-muted);
  }

  .section-right {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 4px;
  }

  .section-right::before {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--border-muted);
  }

  .section-divider {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .divider-label {
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    text-transform: uppercase;
  }

  .count-capsule {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 16px;
    height: 14px;
    padding: 0 4px;
    background-color: var(--bg-primary);
    color: var(--text-primary);
    border-radius: 7px;
    font-size: 9px;
    font-weight: 600;
  }

  .tree-section {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .tree-children {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .tree-item-wrapper {
    margin: 0;
    padding: 0;
  }

  .tree-item {
    display: flex;
    align-items: center;
    width: calc(100% - 8px);
    padding: 3px 8px;
    font-size: var(--size-md);
    gap: 4px;
    cursor: pointer;
    position: relative;
    border-radius: 6px;
    margin: 0 4px;
    background: none;
    border: none;
    text-align: left;
    color: inherit;
    font-family: inherit;
    transition:
      background-color 0.1s,
      box-shadow 0.1s;
  }

  .tree-item:hover {
    background-color: var(--bg-hover);
  }

  .tree-item.selected {
    background-color: var(--bg-primary);
    box-shadow: inset 2px 0 0 var(--accent-primary);
  }

  .tree-item.selected .file-name {
    color: var(--text-primary);
    font-weight: 500;
  }

  .dir-item {
    color: var(--text-muted);
  }

  .dir-chevron {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 14px;
  }

  .dir-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .dir-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file-item {
    gap: 6px;
  }

  .file-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    color: var(--text-primary);
  }

  .status-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .comment-indicator {
    display: flex;
    align-items: center;
    color: var(--text-muted);
    flex-shrink: 0;
    margin-left: auto;
    padding-left: 4px;
  }

  /* Search-related styles */
  .search-chevron {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 14px;
    cursor: pointer;
    color: var(--text-muted);
    transition: color 0.1s;
  }

  .search-chevron:hover {
    color: var(--text-primary);
  }

  .search-spacer {
    width: 14px;
    flex-shrink: 0;
  }

  .search-result-count {
    display: flex;
    align-items: center;
    justify-content: center;
    min-width: 20px;
    height: 16px;
    padding: 0 4px;
    margin-left: auto;
    background-color: var(--accent-primary-muted, rgba(59, 130, 246, 0.15));
    color: var(--accent-primary);
    border-radius: 8px;
    font-size: var(--size-xs);
    font-weight: 500;
    flex-shrink: 0;
  }

  .has-search-results {
    /* Optional: add subtle styling for files with search results */
  }

  .comments-section {
    margin-bottom: 8px;
  }

  .comment-item-container {
    position: relative;
    width: 100%;
  }

  .comment-item {
    position: relative;
    flex-direction: column;
    align-items: flex-start !important;
    gap: 2px !important;
    padding-top: 6px !important;
    padding-bottom: 6px !important;
    padding-left: 28px !important;
    width: 100%;
  }

  .comment-icon {
    position: absolute;
    left: 8px;
    top: 8px;
    color: var(--text-faint);
  }

  .comment-details {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    min-width: 0;
    padding-right: 32px;
  }

  .comment-location {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-xs);
  }

  .comment-file {
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .comment-line {
    flex-shrink: 0;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-faint);
  }

  .comment-preview {
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .comment-delete-btn {
    position: absolute;
    right: 12px;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    opacity: 0;
    transition:
      opacity 0.1s,
      color 0.1s;
    z-index: 1;
  }

  .comment-item-container:hover .comment-delete-btn {
    opacity: 1;
  }

  .comment-delete-btn:hover {
    color: var(--status-deleted);
  }

  .copy-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .copy-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .copy-btn.copied {
    color: var(--status-added);
  }
</style>
