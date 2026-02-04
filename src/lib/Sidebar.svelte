<!--
  Sidebar.svelte - File tree with review workflow
  
  Shows files changed in the current diff (base..head) as a collapsible tree.
  Files needing review appear above the divider.
  Reviewed files appear below the divider.
  Review state comes from the shared commentsState store (single source of truth).
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    MessageSquare,
    CircleFadingArrowUp,
    CircleFadingPlus,
    CircleArrowUp,
    CirclePlus,
    CircleMinus,
    CircleX,
    Check,
    RotateCcw,
    ChevronRight,
    ChevronDown,
    Folder,
    List,
    FolderTree,
    Eye,
    X,
    Plus,
    Wand2,
    Trash2,
    GitCommitHorizontal,
    Orbit,
    Copy,
    GitCompareArrows,
    GitPullRequest,
    Settings2,
    AlertCircle,
  } from 'lucide-svelte';
  import {
    commentsState,
    toggleReviewed as toggleReviewedAction,
    deleteComment,
    copyCommentsToClipboard,
    deleteAllComments,
  } from './stores/comments.svelte';
  import { registerShortcuts } from './services/keyboard';
  import { referenceFilesState } from './stores/referenceFiles.svelte';
  import { preferences } from './stores/preferences.svelte';
  import AgentPanel from './features/agent/AgentPanel.svelte';
  import CommitModal from './CommitModal.svelte';
  import DiffSelectorModal from './DiffSelectorModal.svelte';
  import PRSelectorModal from './PRSelectorModal.svelte';
  import { DiffSpec, gitRefDisplay } from './types';
  import type {
    DiffSpec as DiffSpecType,
    FileDiffSummary,
    FileDiff,
    ChangesetSummary,
  } from './types';
  import type { AgentState, Artifact } from './stores/agent.svelte';
  import { repoState } from './stores/repoState.svelte';
  import {
    getPresets,
    diffSelection,
    getDisplayLabel,
    type DiffPreset,
  } from './stores/diffSelection.svelte';
  import {
    smartDiffState,
    checkAi,
    runAnalysis,
    clearResults as clearSmartDiffState,
    toggleAnnotations,
    clearAnalysisError,
  } from './stores/smartDiff.svelte';
  import { saveArtifact } from './services/review';
  import CrossFileSearchBar from './CrossFileSearchBar.svelte';
  import SearchResultItem from './SearchResultItem.svelte';
  import {
    globalSearchState,
    openSearch,
    goToNextResult,
    goToPrevResult,
    isCurrentResult,
    getGlobalIndex,
    setCurrentResult,
    expandFileResults,
    collapseFileResults,
    getFlattenedResults,
  } from './stores/globalSearch.svelte';
  import { diffState, loadFileDiff } from './stores/diffState.svelte';
  import { getTextLines } from './diffUtils';

  interface FileEntry {
    path: string;
    status: 'added' | 'deleted' | 'modified' | 'renamed';
    isReviewed: boolean;
    commentCount: number;
  }

  interface TreeNode {
    name: string;
    path: string;
    isDir: boolean;
    children: TreeNode[];
    file?: FileEntry;
  }

  interface Props {
    /** File summaries from list_diff_files */
    files: FileDiffSummary[];
    /** Whether the file list is loading */
    loading?: boolean;
    /** Called when user selects a file to view, optionally scrolling to a line and/or expanding a comment */
    onFileSelect?: (path: string, scrollToLine?: number, commentId?: string) => void;
    /** Currently selected file path */
    selectedFile?: string | null;
    /** Whether we're viewing the working tree */
    isWorkingTree?: boolean;
    /** Called when user wants to add a reference file */
    onAddReferenceFile?: () => void;
    /** Called when user wants to remove a reference file */
    onRemoveReferenceFile?: (path: string) => void;
    /** Repository path for AI agent */
    repoPath?: string | null;
    /** Current diff spec for artifact persistence */
    spec?: DiffSpecType | null;
    /** Agent state for this tab's chat session */
    agentState?: AgentState | null;
    /** Called after a successful commit */
    onCommit?: () => void;
    /**
     * Callback to reload comments for a specific tab after AI analysis.
     * @param spec - The diff spec to load comments for
     * @param repoPath - The repo path
     */
    onReloadCommentsForTab?: (spec: DiffSpecType, repoPath: string | null) => Promise<void>;
    /** Called when user selects a preset diff */
    onPresetSelect?: (preset: DiffPreset) => void;
    /** Called when user selects a custom diff (from modal or PR selector) */
    onCustomDiff?: (spec: DiffSpecType, label?: string, prNumber?: number) => Promise<void>;
  }

  let {
    files,
    loading = false,
    onFileSelect,
    selectedFile = null,
    isWorkingTree = true,
    onAddReferenceFile,
    onRemoveReferenceFile,
    repoPath = null,
    spec = null,
    agentState = null,
    onCommit,
    onReloadCommentsForTab,
    onPresetSelect,
    onCustomDiff,
  }: Props = $props();

  let collapsedDirs = $state(new Set<string>());
  let collapsedSearchResults = $state(new Set<string>());
  let treeView = $state(false);
  let showCommitModal = $state(false);
  let copiedFeedback = $state(false);

  // Diff selector state
  let diffDropdownOpen = $state(false);
  let showCustomModal = $state(false);
  let showPRModal = $state(false);

  // Commit button state
  let canCommit = $derived(isWorkingTree && files.length > 0);

  // Get current display label for diff selector
  let currentLabel = $derived(getDisplayLabel());

  // Check if current selection matches a preset
  function isPresetSelected(preset: DiffPreset): boolean {
    return DiffSpec.display(preset.spec) === DiffSpec.display(diffSelection.spec);
  }

  // Get display string for a DiffSpec in the dropdown
  function getSpecDisplay(spec: DiffSpecType): string {
    return DiffSpec.display(spec);
  }

  // Get initial base string for the custom modal
  function getInitialBase(): string {
    return gitRefDisplay(diffSelection.spec.base);
  }

  // Get initial head string for the custom modal
  function getInitialHead(): string {
    return gitRefDisplay(diffSelection.spec.head);
  }

  function handlePresetSelect(preset: DiffPreset) {
    diffDropdownOpen = false;
    onPresetSelect?.(preset);
  }

  function handleCustomClick() {
    diffDropdownOpen = false;
    showCustomModal = true;
  }

  function handlePRClick() {
    diffDropdownOpen = false;
    showPRModal = true;
  }

  async function handlePRSubmit(spec: DiffSpecType, label: string, prNumber: number) {
    // Modal will close itself after this completes
    await onCustomDiff?.(spec, label, prNumber);
  }

  function handleCustomSubmit(spec: DiffSpecType) {
    showCustomModal = false;
    onCustomDiff?.(spec);
  }

  // Close dropdown when clicking outside
  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (!target.closest('.diff-selector')) {
      diffDropdownOpen = false;
    }
  }

  // AI analysis state
  let isAiLoading = $derived(smartDiffState.loading);
  let canRunAi = $derived(files.length > 0 && !loading);
  let hasFileAnnotations = $derived(smartDiffState.results.size > 0);
  let showAnnotations = $derived(smartDiffState.showAnnotations);

  /**
   * Get the primary path for a file summary.
   */
  function getFilePath(summary: FileDiffSummary): string {
    return summary.after ?? summary.before ?? '';
  }

  /**
   * Determine file status from a FileDiffSummary.
   */
  function getFileStatus(summary: FileDiffSummary): 'added' | 'deleted' | 'modified' | 'renamed' {
    if (summary.before === null) return 'added';
    if (summary.after === null) return 'deleted';
    if (summary.before !== summary.after) return 'renamed';
    return 'modified';
  }

  /**
   * Build file list from summaries with review state.
   * Uses commentsState for both comment counts and reviewed status (reactive).
   */
  function buildFileList(
    fileSummaries: FileDiffSummary[],
    reviewedPaths: string[],
    comments: typeof commentsState.comments
  ): FileEntry[] {
    const reviewedSet = new Set(reviewedPaths);

    // Count comments per file from the shared comments state
    const commentCounts = new Map<string, number>();
    for (const comment of comments) {
      commentCounts.set(comment.path, (commentCounts.get(comment.path) || 0) + 1);
    }

    return fileSummaries.map((summary) => {
      const path = getFilePath(summary);
      return {
        path,
        status: getFileStatus(summary),
        isReviewed: reviewedSet.has(path),
        commentCount: commentCounts.get(path) || 0,
      };
    });
  }

  /**
   * Build a tree structure from a flat list of files.
   */
  function buildTree(fileEntries: FileEntry[]): TreeNode[] {
    const root: TreeNode[] = [];

    for (const file of fileEntries) {
      const parts = file.path.split('/');
      let currentLevel = root;

      for (let i = 0; i < parts.length; i++) {
        const part = parts[i];
        const isLast = i === parts.length - 1;
        const pathSoFar = parts.slice(0, i + 1).join('/');

        let existing = currentLevel.find((n) => n.name === part);

        if (!existing) {
          existing = {
            name: part,
            path: pathSoFar,
            isDir: !isLast,
            children: [],
            file: isLast ? file : undefined,
          };
          currentLevel.push(existing);
        }

        if (!isLast) {
          currentLevel = existing.children;
        }
      }
    }

    // Sort: directories first, then alphabetically
    function sortTree(nodes: TreeNode[]): TreeNode[] {
      nodes.sort((a, b) => {
        if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
        return a.name.localeCompare(b.name);
      });
      for (const node of nodes) {
        if (node.children.length > 0) {
          sortTree(node.children);
        }
      }
      return nodes;
    }

    return sortTree(root);
  }

  /**
   * Flatten single-child directory chains into combined paths.
   * e.g., src/lib/components becomes a single "src/lib/components" node if each has one child.
   */
  function compactTree(nodes: TreeNode[]): TreeNode[] {
    return nodes.map((node) => {
      if (node.isDir && node.children.length === 1 && node.children[0].isDir) {
        // Merge with single child
        const child = compactTree(node.children)[0];
        return {
          ...child,
          name: node.name + '/' + child.name,
          path: child.path,
        };
      }
      return {
        ...node,
        children: node.isDir ? compactTree(node.children) : [],
      };
    });
  }

  // Use commentsState for both comments and reviewed paths (single source of truth)
  let fileEntries = $derived(
    buildFileList(files, commentsState.reviewedPaths, commentsState.comments)
  );
  let needsReview = $derived(fileEntries.filter((f) => !f.isReviewed));
  let reviewed = $derived(fileEntries.filter((f) => f.isReviewed));

  // Build trees for each section
  let needsReviewTree = $derived(compactTree(buildTree(needsReview)));
  let reviewedTree = $derived(compactTree(buildTree(reviewed)));

  /**
   * Extract files from tree nodes in display order (depth-first).
   */
  function extractFilesFromTree(nodes: TreeNode[]): FileEntry[] {
    const result: FileEntry[] = [];
    for (const node of nodes) {
      if (node.isDir) {
        result.push(...extractFilesFromTree(node.children));
      } else if (node.file) {
        result.push(node.file);
      }
    }
    return result;
  }

  /**
   * Get files in the order they're displayed in the sidebar.
   */
  let displayOrderedFiles = $derived.by(() => {
    if (treeView) {
      // In tree view, extract files from trees in display order
      return [...extractFilesFromTree(needsReviewTree), ...extractFilesFromTree(reviewedTree)];
    } else {
      // In flat view, use the existing order
      return [...needsReview, ...reviewed];
    }
  });

  /**
   * Get FileDiffSummary array in display order for search navigation.
   */
  let displayOrderedFileSummaries = $derived.by(() => {
    const orderedPaths = displayOrderedFiles.map((f) => f.path);
    // Create a map for quick lookup
    const fileMap = new Map(files.map((f) => [getFilePath(f), f]));
    // Return files in display order
    return orderedPaths.map((path) => fileMap.get(path)).filter(Boolean) as FileDiffSummary[];
  });

  async function selectFile(file: FileEntry) {
    // If search is active and file has results, jump to first result
    if (globalSearchState.isOpen && globalSearchState.query) {
      const fileResult = globalSearchState.fileResults.get(file.path);
      if (fileResult && fileResult.matches.length > 0) {
        // Get the first match in this file
        const firstMatch = fileResult.matches[0];

        // Find the global index for this match
        const globalIndex = getGlobalIndex(displayOrderedFileSummaries, file.path, 0);

        // Set as current result
        if (globalIndex !== -1) {
          setCurrentResult(globalIndex);
        }

        // Auto-expand search results if collapsed
        if (areSearchResultsCollapsed(file.path)) {
          await toggleSearchResults(file.path);
        }

        // Select file and scroll to the first match
        onFileSelect?.(file.path, firstMatch.lineIndex);
        return;
      }
    }

    // Normal file selection (no search or no results)
    onFileSelect?.(file.path);
  }

  async function toggleReviewed(event: MouseEvent | KeyboardEvent, file: FileEntry) {
    event.stopPropagation();
    await toggleReviewedAction(file.path);
  }

  async function handleDeleteComment(event: MouseEvent, commentId: string) {
    event.stopPropagation();
    await deleteComment(commentId);
  }

  async function handleCopyComments() {
    const success = await copyCommentsToClipboard();
    if (success) {
      copiedFeedback = true;
      setTimeout(() => {
        copiedFeedback = false;
      }, 1500);
    }
  }

  function toggleDir(path: string) {
    const newSet = new Set(collapsedDirs);
    if (newSet.has(path)) {
      newSet.delete(path);
    } else {
      newSet.add(path);
    }
    collapsedDirs = newSet;
  }

  function isCollapsed(path: string): boolean {
    return collapsedDirs.has(path);
  }

  async function toggleSearchResults(path: string) {
    const newSet = new Set(collapsedSearchResults);
    if (newSet.has(path)) {
      // Expanding - load diff if not cached for snippet generation
      newSet.delete(path);

      // Ensure diff is loaded so getMatchSnippet can access it
      if (!diffState.diffCache.has(path)) {
        await loadFileDiff(path);
      }
    } else {
      // Collapsing
      newSet.add(path);
    }
    collapsedSearchResults = newSet;
  }

  function areSearchResultsCollapsed(path: string): boolean {
    return collapsedSearchResults.has(path);
  }

  /**
   * Get just the filename from a full path.
   */
  function getFileName(path: string): string {
    return path.split('/').pop() || path;
  }

  /**
   * Format line range for display.
   */
  function formatLineRange(span: { start: number; end: number }): string {
    if (span.end === span.start + 1) {
      return `L${span.start + 1}`;
    }
    return `L${span.start + 1}-${span.end}`;
  }

  /**
   * Truncate text for preview.
   */
  function truncateText(text: string, maxLength = 40): string {
    const singleLine = text.replace(/\n/g, ' ').trim();
    if (singleLine.length <= maxLength) return singleLine;
    return singleLine.slice(0, maxLength).trim() + '...';
  }

  // ==========================================================================
  // Search-related helpers
  // ==========================================================================

  /**
   * Get a snippet of text around a search match.
   */
  function getMatchSnippet(
    match: import('./services/diffSearch').SearchMatch,
    filePath: string
  ): string {
    const diff = diffState.diffCache.get(filePath);
    if (!diff) return '';

    const afterLines = getTextLines(diff, 'after');

    // Search only looks at right/after side
    const line = afterLines[match.lineIndex];
    const location = match.right;

    if (!line || !location) return '';

    // Extract context around match
    const start = Math.max(0, location.startCol - 20);
    const end = Math.min(line.length, location.endCol + 30);

    let snippet = line.slice(start, end).trim();
    if (start > 0) snippet = '...' + snippet;
    if (end < line.length) snippet = snippet + '...';

    return snippet;
  }

  /**
   * Handle clicking a search result.
   */
  async function handleSearchResultClick(
    filePath: string,
    match: import('./services/diffSearch').SearchMatch,
    globalIndex: number
  ) {
    // Update current result index
    setCurrentResult(globalIndex);

    // Auto-expand search results for this file
    if (areSearchResultsCollapsed(filePath)) {
      await toggleSearchResults(filePath);
    }

    // Select the file and scroll to the match
    onFileSelect?.(filePath, match.lineIndex);
  }

  /**
   * Navigate to next search result.
   */
  async function handleNextResult() {
    if (!globalSearchState.isOpen || globalSearchState.totalMatches === 0) return;

    const result = await goToNextResult(displayOrderedFileSummaries, loadFileDiff);
    if (result) {
      // Auto-expand search results for this file
      if (areSearchResultsCollapsed(result.filePath)) {
        await toggleSearchResults(result.filePath);
      }
      onFileSelect?.(result.filePath, result.match.lineIndex);
    }
  }

  /**
   * Navigate to previous search result.
   */
  async function handlePrevResult() {
    if (!globalSearchState.isOpen || globalSearchState.totalMatches === 0) return;

    const result = await goToPrevResult(displayOrderedFileSummaries, loadFileDiff);
    if (result) {
      // Auto-expand search results for this file
      if (areSearchResultsCollapsed(result.filePath)) {
        await toggleSearchResults(result.filePath);
      }
      onFileSelect?.(result.filePath, result.match.lineIndex);
    }
  }

  // Initialize all search results as collapsed when search completes, except the first result's file
  $effect(() => {
    if (globalSearchState.isOpen && globalSearchState.fileResults.size > 0) {
      const flattened = getFlattenedResults(displayOrderedFileSummaries);
      const firstResultPath = flattened.length > 0 ? flattened[0].filePath : null;

      const newCollapsed = new Set<string>();
      for (const filePath of globalSearchState.fileResults.keys()) {
        // Collapse all except the first result's file
        if (filePath !== firstResultPath) {
          newCollapsed.add(filePath);
        }
      }
      collapsedSearchResults = newCollapsed;
    }
  });

  // Get flat list of file paths in display order
  function getFilePaths(): string[] {
    return files.map((f) => getFilePath(f));
  }

  // Navigate to next file
  function goToNextFile(): void {
    const paths = getFilePaths();
    if (paths.length === 0) return;

    const currentIndex = selectedFile ? paths.indexOf(selectedFile) : -1;
    const nextIndex = currentIndex < paths.length - 1 ? currentIndex + 1 : 0;
    onFileSelect?.(paths[nextIndex]);
  }

  // Navigate to previous file
  function goToPrevFile(): void {
    const paths = getFilePaths();
    if (paths.length === 0) return;

    const currentIndex = selectedFile ? paths.indexOf(selectedFile) : 0;
    const prevIndex = currentIndex > 0 ? currentIndex - 1 : paths.length - 1;
    onFileSelect?.(paths[prevIndex]);
  }

  /**
   * Handle AI analysis button click.
   * Triggers analysis if not already running.
   */
  async function handleAiAnalysis() {
    // If already loading, do nothing (button shows progress)
    if (isAiLoading) return;

    if (!canRunAi) return;

    // Check AI availability first
    const available = await checkAi();
    if (!available) {
      // Ensure error is shown (might have been dismissed from previous attempt)
      if (!smartDiffState.aiError) {
        smartDiffState.aiError = 'No AI tool available. Please install Claude CLI or Goose.';
      }
      return;
    }

    // Start analysis in background
    runChangesetAnalysis();
  }

  /**
   * Create an artifact from a changeset summary.
   */
  function createArtifactFromSummary(summary: ChangesetSummary): Artifact {
    // Generate title from summary (first 50 chars)
    const title = summary.summary
      .replace(/^#+\s*/, '') // Strip markdown headers
      .substring(0, 50)
      .trim();

    // Format as markdown document
    let content = '';

    if (summary.summary) {
      content += `# Summary\n\n${summary.summary}\n\n`;
    }

    if (summary.key_changes.length > 0) {
      content += `# Key Changes\n\n`;
      for (const change of summary.key_changes) {
        content += `- ${change}\n`;
      }
      content += '\n';
    }

    if (summary.concerns.length > 0) {
      content += `# Concerns\n\n`;
      for (const concern of summary.concerns) {
        content += `- ${concern}\n`;
      }
    }

    return {
      id: crypto.randomUUID(),
      title: `AI Review: ${title}`,
      content: content.trim(),
      createdAt: new Date().toISOString(),
    };
  }

  /**
   * Run changeset analysis in background.
   * Automatically saves results as an artifact when complete.
   * Sets agentState.loading to show busy indicators in AgentPanel and tab bar.
   */
  async function runChangesetAnalysis() {
    // Capture context at call time for the analysis request
    const capturedAgentState = agentState;
    const capturedRepoPath = repoState.currentPath ?? null;
    const capturedSpec = diffSelection.spec;

    // Set agent loading state to show "Working on it..." and tab spinner
    if (capturedAgentState) {
      capturedAgentState.loading = true;
    }

    try {
      // Single call - backend handles file listing and content loading
      const result = await runAnalysis(capturedRepoPath, capturedSpec);

      if (result) {
        // Reload comments for the tab where analysis was started
        await onReloadCommentsForTab?.(capturedSpec, capturedRepoPath);

        // Auto-save artifact if there's a changeset summary
        if (smartDiffState.changesetSummary && capturedAgentState) {
          const artifact = createArtifactFromSummary(smartDiffState.changesetSummary);
          // Save to database
          await saveArtifact(capturedSpec, artifact, capturedRepoPath ?? undefined);
          // Add to UI immediately
          capturedAgentState.artifacts.push(artifact);
        }
      }
    } catch (e) {
      console.error('Analysis failed:', e);
    } finally {
      // Clear agent loading state on the captured state (correct tab)
      if (capturedAgentState) {
        capturedAgentState.loading = false;
      }
    }
  }

  // Register keyboard shortcuts
  onMount(() => {
    const unregister = registerShortcuts([
      {
        id: 'copy-file-path',
        keys: ['c'],
        modifiers: { meta: true, shift: true },
        description: 'Copy file path',
        category: 'files',
        handler: () => {
          if (selectedFile) {
            navigator.clipboard.writeText(selectedFile).catch((err) => {
              console.error('Failed to copy file path:', err);
            });
          }
        },
      },
      {
        id: 'next-file',
        keys: [']'],
        modifiers: { meta: true },
        description: 'Next file',
        category: 'files',
        handler: goToNextFile,
      },
      {
        id: 'prev-file',
        keys: ['['],
        modifiers: { meta: true },
        description: 'Previous file',
        category: 'files',
        handler: goToPrevFile,
      },
      {
        id: 'global-search-open',
        keys: ['f'],
        modifiers: { meta: true },
        description: 'Open search',
        category: 'search',
        allowInInputs: true,
        handler: () => openSearch(),
      },
      {
        id: 'global-search-next',
        keys: ['g'],
        modifiers: { meta: true },
        description: 'Next search result',
        category: 'search',
        allowInInputs: true,
        handler: handleNextResult,
      },
      {
        id: 'global-search-prev',
        keys: ['g'],
        modifiers: { meta: true, shift: true },
        description: 'Previous search result',
        category: 'search',
        allowInInputs: true,
        handler: handlePrevResult,
      },
      {
        id: 'copy-comments',
        keys: ['c'],
        description: 'Copy all comments',
        category: 'comments',
        handler: () => {
          if (commentsState.comments.length > 0) {
            handleCopyComments();
          }
        },
      },
    ]);

    return () => unregister();
  });
</script>

{#snippet fileIcon(file: FileEntry, showReviewedSection: boolean)}
  <span
    class="status-icon"
    onclick={(e) => toggleReviewed(e, file)}
    onkeydown={(e) => e.key === 'Enter' && toggleReviewed(e, file)}
    role="button"
    tabindex="0"
    title={showReviewedSection ? 'Mark as needs review' : 'Mark as reviewed'}
  >
    <!-- Default icon (hidden on hover) -->
    <span class="icon-default">
      {#if file.status === 'added'}
        {#if isWorkingTree}
          <CircleFadingPlus size={16} />
        {:else}
          <CirclePlus size={16} />
        {/if}
      {:else if file.status === 'deleted'}
        {#if isWorkingTree}
          <CircleX size={16} />
        {:else}
          <CircleMinus size={16} />
        {/if}
      {:else if isWorkingTree}
        <CircleFadingArrowUp size={16} />
      {:else}
        <CircleArrowUp size={16} />
      {/if}
    </span>
    <!-- Hover icon -->
    <span class="icon-hover" class:icon-hover-unreview={showReviewedSection}>
      {#if showReviewedSection}
        <RotateCcw size={16} />
      {:else}
        <Check size={16} />
      {/if}
    </span>
  </span>
{/snippet}

{#snippet treeNodes(nodes: TreeNode[], depth: number, showReviewedSection: boolean)}
  {#each nodes as node (node.path)}
    {#if node.isDir}
      <!-- Directory node -->
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
          <span class="dir-icon">
            <Folder size={14} />
          </span>
          <span class="dir-name">{node.name}</span>
        </button>
        {#if !isCollapsed(node.path)}
          <ul class="tree-children">
            {@render treeNodes(node.children, depth + 1, showReviewedSection)}
          </ul>
        {/if}
      </li>
    {:else if node.file}
      <!-- File node -->
      <li class="tree-item-wrapper">
        <button
          class="tree-item file-item"
          class:selected={selectedFile === node.file.path}
          class:has-search-results={globalSearchState.isOpen &&
            globalSearchState.fileResults.has(node.file.path)}
          style="padding-left: {8 + depth * 12}px"
          onclick={() => selectFile(node.file!)}
        >
          {#if globalSearchState.isOpen}
            {#if globalSearchState.fileResults.has(node.file.path)}
              <span
                class="dir-chevron"
                onclick={(e) => {
                  e.stopPropagation();
                  toggleSearchResults(node.file!.path);
                }}
                role="button"
                tabindex="0"
                onkeydown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    e.stopPropagation();
                    toggleSearchResults(node.file!.path);
                  }
                }}
              >
                {#if areSearchResultsCollapsed(node.file.path)}
                  <ChevronRight size={14} />
                {:else}
                  <ChevronDown size={14} />
                {/if}
              </span>
            {:else}
              <span class="search-spacer"></span>
            {/if}
          {/if}
          {@render fileIcon(node.file, showReviewedSection)}
          <span class="file-name">{node.name}</span>
          {#if node.file.commentCount > 0}
            <span class="comment-indicator">
              <MessageSquare size={12} />
            </span>
          {/if}
          {#if globalSearchState.isOpen && globalSearchState.fileResults.has(node.file.path)}
            {@const resultCount =
              globalSearchState.fileResults.get(node.file.path)?.matches.length ?? 0}
            <span
              class="search-result-count"
              title="{resultCount} search result{resultCount !== 1 ? 's' : ''}"
            >
              {resultCount}
            </span>
          {/if}
        </button>

        <!-- Search results (if search is active and this file has matches) -->
        {#if globalSearchState.isOpen && globalSearchState.fileResults.has(node.file.path) && !areSearchResultsCollapsed(node.file.path)}
          {@const fileResult = globalSearchState.fileResults.get(node.file.path)}
          {#if fileResult}
            <div class="search-results-container" style="margin-left: {8 + (depth + 1) * 12}px">
              {#each fileResult.matches.slice(0, fileResult.displayLimit) as match, i}
                {@const snippet = getMatchSnippet(match, node.file.path)}
                {@const isCurrent = isCurrentResult(displayOrderedFileSummaries, node.file.path, i)}
                {@const globalIndex = getGlobalIndex(
                  displayOrderedFileSummaries,
                  node.file.path,
                  i
                )}
                <SearchResultItem
                  {match}
                  {snippet}
                  {isCurrent}
                  onclick={() => handleSearchResultClick(node.file!.path, match, globalIndex)}
                />
              {/each}

              {#if fileResult.matches.length > fileResult.displayLimit}
                <button class="show-more-btn" onclick={() => expandFileResults(node.file!.path)}>
                  Show {fileResult.matches.length - fileResult.displayLimit} more
                </button>
              {:else if fileResult.displayLimit > 5}
                <button class="show-less-btn" onclick={() => collapseFileResults(node.file!.path)}>
                  Show less
                </button>
              {/if}
            </div>
          {/if}
        {/if}
      </li>
    {/if}
  {/each}
{/snippet}

{#snippet flatFileList(fileList: FileEntry[], showReviewedSection: boolean)}
  {#each fileList as file (file.path)}
    <li class="tree-item-wrapper">
      <button
        class="tree-item file-item"
        class:selected={selectedFile === file.path}
        class:has-search-results={globalSearchState.isOpen &&
          globalSearchState.fileResults.has(file.path)}
        style="padding-left: 8px"
        onclick={() => selectFile(file)}
        title={file.path}
      >
        {#if globalSearchState.isOpen}
          {#if globalSearchState.fileResults.has(file.path)}
            <span
              class="dir-chevron"
              onclick={(e) => {
                e.stopPropagation();
                toggleSearchResults(file.path);
              }}
              role="button"
              tabindex="0"
              onkeydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  e.stopPropagation();
                  toggleSearchResults(file.path);
                }
              }}
            >
              {#if areSearchResultsCollapsed(file.path)}
                <ChevronRight size={14} />
              {:else}
                <ChevronDown size={14} />
              {/if}
            </span>
          {:else}
            <span class="search-spacer"></span>
          {/if}
        {/if}
        {@render fileIcon(file, showReviewedSection)}
        <span class="file-name truncate-start">{file.path}</span>
        {#if file.commentCount > 0}
          <span class="comment-indicator">
            <MessageSquare size={12} />
          </span>
        {/if}
        {#if globalSearchState.isOpen && globalSearchState.fileResults.has(file.path)}
          {@const resultCount = globalSearchState.fileResults.get(file.path)?.matches.length ?? 0}
          <span
            class="search-result-count"
            title="{resultCount} search result{resultCount !== 1 ? 's' : ''}"
          >
            {resultCount}
          </span>
        {/if}
      </button>

      <!-- Search results (if search is active and this file has matches) -->
      {#if globalSearchState.isOpen && globalSearchState.fileResults.has(file.path) && !areSearchResultsCollapsed(file.path)}
        {@const fileResult = globalSearchState.fileResults.get(file.path)}
        {#if fileResult}
          <div class="search-results-container" style="margin-left: 20px">
            {#each fileResult.matches.slice(0, fileResult.displayLimit) as match, i}
              {@const snippet = getMatchSnippet(match, file.path)}
              {@const isCurrent = isCurrentResult(displayOrderedFileSummaries, file.path, i)}
              {@const globalIndex = getGlobalIndex(displayOrderedFileSummaries, file.path, i)}
              <SearchResultItem
                {match}
                {snippet}
                {isCurrent}
                onclick={() => handleSearchResultClick(file.path, match, globalIndex)}
              />
            {/each}

            {#if fileResult.matches.length > fileResult.displayLimit}
              <button class="show-more-btn" onclick={() => expandFileResults(file.path)}>
                Show {fileResult.matches.length - fileResult.displayLimit} more
              </button>
            {:else if fileResult.displayLimit > 5}
              <button class="show-less-btn" onclick={() => collapseFileResults(file.path)}>
                Show less
              </button>
            {/if}
          </div>
        {/if}
      {/if}
    </li>
  {/each}
{/snippet}

{#snippet commentList()}
  {#each commentsState.comments as comment (comment.id)}
    <li class="tree-item-wrapper">
      <div class="comment-item-container">
        <button
          class="tree-item comment-item"
          class:ai-comment={comment.author === 'ai'}
          class:category-warning={comment.category === 'warning'}
          class:category-suggestion={comment.category === 'suggestion'}
          class:category-explanation={comment.category === 'explanation'}
          class:category-context={comment.category === 'context'}
          style="padding-left: 8px"
          onclick={() => onFileSelect?.(comment.path, comment.span.start, comment.id)}
        >
          <span class="comment-icon">
            {#if comment.author === 'ai'}
              <Wand2 size={12} />
            {:else}
              <MessageSquare size={12} />
            {/if}
          </span>
          <span class="comment-details">
            <span class="comment-location">
              <span class="comment-file">{getFileName(comment.path)}</span>
              <span class="comment-line">{formatLineRange(comment.span)}</span>
            </span>
            <span class="comment-preview">{truncateText(comment.content)}</span>
          </span>
        </button>
        <button
          class="comment-delete-btn"
          onclick={(e) => handleDeleteComment(e, comment.id)}
          title="Delete comment"
        >
          <Trash2 size={12} />
        </button>
      </div>
    </li>
  {/each}
{/snippet}

<svelte:window onclick={handleClickOutside} />

<div class="sidebar-content">
  <!-- Diff selector -->
  <div class="diff-selector">
    <button
      class="diff-selector-btn"
      onclick={() => (diffDropdownOpen = !diffDropdownOpen)}
      class:open={diffDropdownOpen}
    >
      <GitCompareArrows size={14} />
      <span class="diff-label">{currentLabel}</span>
      <ChevronDown size={12} />
    </button>

    {#if diffDropdownOpen}
      <div class="dropdown diff-dropdown">
        {#each getPresets() as preset}
          <button
            class="dropdown-item diff-item"
            class:active={isPresetSelected(preset)}
            onclick={() => handlePresetSelect(preset)}
          >
            <GitCompareArrows size={14} />
            <div class="diff-item-content">
              <span class="diff-item-label">{preset.label}</span>
              <span class="diff-item-spec">{getSpecDisplay(preset.spec)}</span>
            </div>
          </button>
        {/each}
        <div class="dropdown-divider"></div>
        <button class="dropdown-item custom-item" onclick={handlePRClick}>
          <GitPullRequest size={12} />
          <span>Pull Request...</span>
        </button>
        <button class="dropdown-item custom-item" onclick={handleCustomClick}>
          <Settings2 size={12} />
          <span>Custom range...</span>
        </button>
      </div>
    {/if}
  </div>

  <!-- Search bar -->
  <CrossFileSearchBar {files} {loadFileDiff} />

  {#if loading}
    <div class="loading-state">
      <p>Loading...</p>
    </div>
  {:else}
    <div class="file-list">
      <!-- Needs Review section -->
      {#if needsReview.length > 0}
        <div class="section-header">
          <div class="section-left">
            <button
              class="view-toggle"
              onclick={() => (treeView = !treeView)}
              title={treeView ? 'Switch to flat list' : 'Switch to tree view'}
            >
              {#if treeView}
                <List size={12} />
              {:else}
                <FolderTree size={12} />
              {/if}
            </button>
          </div>
          <div class="section-divider">
            <span class="divider-label">CHANGED</span>
            {#if needsReview.length > 0}
              <span class="count-capsule">{needsReview.length}</span>
            {/if}
          </div>
          <div class="section-right">
            {#if isWorkingTree}
              <button
                class="commit-btn"
                class:disabled={!canCommit}
                onclick={() => canCommit && (showCommitModal = true)}
                title={canCommit ? 'Commit' : 'No staged or unstaged changes'}
                disabled={!canCommit}
              >
                <GitCommitHorizontal size={12} />
              </button>
            {/if}
          </div>
        </div>
        <ul class="tree-section">
          {#if treeView}
            {@render treeNodes(needsReviewTree, 0, false)}
          {:else}
            {@render flatFileList(needsReview, false)}
          {/if}
        </ul>
      {/if}

      <!-- Divider with REVIEWED label -->
      {#if reviewed.length > 0}
        <div class="section-header">
          <div class="section-left">
            <button
              class="view-toggle"
              onclick={() => (treeView = !treeView)}
              title={treeView ? 'Switch to flat list' : 'Switch to tree view'}
            >
              {#if treeView}
                <List size={12} />
              {:else}
                <FolderTree size={12} />
              {/if}
            </button>
          </div>
          <div class="section-divider">
            <span class="divider-label">REVIEWED</span>
            {#if reviewed.length > 0}
              <span class="count-capsule">{reviewed.length}</span>
            {/if}
          </div>
          <div class="section-right">
            <!-- Show commit button here only if CHANGED section is empty (this is the first section) -->
            {#if isWorkingTree && needsReview.length === 0}
              <button
                class="commit-btn"
                class:disabled={!canCommit}
                onclick={() => canCommit && (showCommitModal = true)}
                title={canCommit ? 'Commit' : 'No staged or unstaged changes'}
                disabled={!canCommit}
              >
                <GitCommitHorizontal size={12} />
              </button>
            {/if}
          </div>
        </div>
      {/if}

      <!-- Reviewed section -->
      {#if reviewed.length > 0}
        <ul class="tree-section reviewed-section">
          {#if treeView}
            {@render treeNodes(reviewedTree, 0, true)}
          {:else}
            {@render flatFileList(reviewed, true)}
          {/if}
        </ul>
      {/if}

      <!-- Reference Files section -->
      <div class="section-header">
        <div class="section-left"></div>
        <div class="section-divider">
          <span class="divider-label">REFERENCE</span>
          {#if referenceFilesState.files.length > 0}
            <span class="count-capsule">{referenceFilesState.files.length}</span>
          {/if}
        </div>
        <div class="section-right">
          <button
            class="add-file-btn"
            onclick={() => onAddReferenceFile?.()}
            title="Add reference file (Cmd+O)"
          >
            <Plus size={12} />
          </button>
        </div>
      </div>
      {#if referenceFilesState.files.length > 0}
        <ul class="tree-section reference-section">
          {#each referenceFilesState.files as refFile (refFile.path)}
            <li class="tree-item-wrapper">
              <div
                class="tree-item file-item reference-item"
                class:selected={selectedFile === refFile.path}
                style="padding-left: 8px"
                role="button"
                tabindex="0"
                onclick={() => onFileSelect?.(refFile.path)}
                onkeydown={(e) => e.key === 'Enter' && onFileSelect?.(refFile.path)}
                title={refFile.path}
              >
                <span class="reference-icon">
                  <Eye size={16} />
                </span>
                <span class="file-name truncate-start">{refFile.path}</span>
                <button
                  class="remove-btn"
                  onclick={(e) => {
                    e.stopPropagation();
                    onRemoveReferenceFile?.(refFile.path);
                  }}
                  onkeydown={(e) => e.key === 'Enter' && e.stopPropagation()}
                  title="Remove reference file"
                >
                  <X size={12} />
                </button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}

      <!-- Comments section -->
      <div class="section-header comments-header">
        <div class="section-left"></div>
        <div class="section-divider">
          <span class="divider-label">COMMENTS</span>
          {#if commentsState.comments.length > 0}
            <span class="count-capsule">{commentsState.comments.length}</span>
          {/if}
        </div>
        <div class="section-right">
          {#if commentsState.comments.length > 0}
            <button
              class="copy-btn"
              class:copied={copiedFeedback}
              onclick={handleCopyComments}
              title="Copy all comments (c)"
            >
              {#if copiedFeedback}
                <Check size={12} />
              {:else}
                <Copy size={12} />
              {/if}
            </button>
            <button class="delete-all-btn" onclick={deleteAllComments} title="Delete all comments">
              <Trash2 size={12} />
            </button>
          {/if}
        </div>
      </div>
      {#if commentsState.comments.length > 0}
        <ul class="tree-section comments-section">
          {@render commentList()}
        </ul>
      {/if}

      <!-- Agent Chat section -->
      {#if agentState}
        <div class="section-header agent-header">
          <div class="section-left"></div>
          <div class="section-divider">
            <span class="divider-label">AGENT</span>
          </div>
          <div class="section-right">
            {#if hasFileAnnotations}
              <button
                class="view-ai-btn"
                class:active={showAnnotations && smartDiffState.annotationsRevealed}
                onclick={() => {
                  // Toggle both showAnnotations and annotationsRevealed together
                  // so clicking the eye directly shows/hides the overlay
                  const newState = !smartDiffState.annotationsRevealed;
                  if (newState) {
                    // Turn on: ensure both flags are true
                    if (!smartDiffState.showAnnotations) toggleAnnotations();
                    smartDiffState.annotationsRevealed = true;
                  } else {
                    // Turn off: just hide the reveal
                    smartDiffState.annotationsRevealed = false;
                  }
                }}
                title={smartDiffState.annotationsRevealed
                  ? 'Hide AI annotations'
                  : 'Show AI annotations'}
              >
                <Eye size={12} />
              </button>
            {/if}
            <button
              class="ai-btn"
              class:loading={isAiLoading}
              class:disabled={!canRunAi}
              onclick={handleAiAnalysis}
              title={isAiLoading ? 'Analyzing...' : 'Analyze with AI'}
              disabled={!canRunAi}
            >
              <div class="ai-icon" class:spinning={isAiLoading}>
                <Orbit size={12} />
              </div>
            </button>
          </div>
        </div>
      {/if}
    </div>

    <!-- Agent Panel outside file-list for flex layout (takes remaining space) -->
    {#if agentState}
      <AgentPanel {repoPath} {spec} {files} {selectedFile} {agentState} />
    {/if}
  {/if}
</div>

{#if showCommitModal}
  <CommitModal
    repoPath={repoState.currentPath}
    onCommit={() => {
      showCommitModal = false;
      onCommit?.();
    }}
    onClose={() => (showCommitModal = false)}
  />
{/if}

{#if showCustomModal}
  <DiffSelectorModal
    initialBase={getInitialBase()}
    initialHead={getInitialHead()}
    onSubmit={handleCustomSubmit}
    onClose={() => (showCustomModal = false)}
  />
{/if}

{#if showPRModal}
  <PRSelectorModal
    repoPath={repoState.currentPath}
    onSubmit={handlePRSubmit}
    onClose={() => (showPRModal = false)}
  />
{/if}

{#if smartDiffState.analysisError || smartDiffState.aiError}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div
    class="error-modal-backdrop"
    role="dialog"
    aria-modal="true"
    aria-labelledby="error-dialog-title"
    onclick={(e) => {
      if (e.target === e.currentTarget) {
        clearAnalysisError();
        smartDiffState.aiError = null;
      }
    }}
    onkeydown={(e) => {
      if (e.key === 'Escape') {
        clearAnalysisError();
        smartDiffState.aiError = null;
      }
    }}
  >
    <div class="error-modal">
      <header class="error-modal-header">
        <h2 id="error-dialog-title">
          <AlertCircle size={18} />
          AI Analysis Failed
        </h2>
      </header>
      <div class="error-modal-body">
        <p class="error-message">{smartDiffState.analysisError || smartDiffState.aiError}</p>
      </div>
      <footer class="error-modal-footer">
        <button
          class="btn btn-primary"
          onclick={() => {
            clearAnalysisError();
            smartDiffState.aiError = null;
          }}
        >
          OK
        </button>
      </footer>
    </div>
  </div>
{/if}

<style>
  .sidebar-content {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
  }

  /* Diff selector */
  .diff-selector {
    position: relative;
    padding: 0 8px;
    flex-shrink: 0;
  }

  /* Reduce top margin of first section after diff selector */
  .diff-selector + .file-list .section-header:first-child {
    margin-top: 8px;
  }

  .diff-selector-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 6px 10px;
    background: var(--bg-primary);
    border: none;
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-xs);
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .diff-selector-btn:hover,
  .diff-selector-btn.open {
    background: var(--bg-hover);
  }

  .diff-selector-btn :global(svg) {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .diff-selector-btn :global(svg:last-child) {
    margin-left: auto;
    transition: transform 0.15s;
  }

  .diff-selector-btn.open :global(svg:last-child) {
    transform: rotate(180deg);
  }

  .diff-label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: left;
  }

  /* Dropdown */
  .dropdown {
    position: absolute;
    top: 100%;
    left: 8px;
    right: 8px;
    margin-top: 4px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    overflow: hidden;
    z-index: 100;
  }

  .diff-dropdown {
    padding-bottom: 4px;
  }

  .dropdown-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 12px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-xs);
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .dropdown-item:hover {
    background-color: var(--bg-hover);
  }

  .dropdown-item.active {
    background-color: var(--bg-primary);
  }

  .diff-item {
    align-items: flex-start;
  }

  .diff-item :global(svg) {
    flex-shrink: 0;
    margin-top: 2px;
  }

  .diff-item-content {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .diff-item-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .diff-item-spec {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dropdown-divider {
    height: 1px;
    background: var(--border-subtle);
    margin: 4px 0;
  }

  .custom-item {
    color: var(--text-muted);
  }

  .custom-item :global(svg) {
    color: var(--text-muted);
  }

  .loading-state {
    padding: 20px 16px;
    text-align: center;
    color: var(--text-muted);
  }

  .view-toggle {
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

  .view-toggle:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .view-toggle:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: -2px;
  }

  .file-list {
    flex-shrink: 0;
    padding: 0;
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

  /* Section header with centered title */
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
    justify-content: flex-start;
    gap: 4px;
  }

  .section-left::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--border-muted);
    margin-left: 4px;
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
    margin-right: 4px;
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

  .reviewed-section {
    opacity: 0.7;
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

  .tree-item:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: -2px;
  }

  /* Directory styling */
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

  .search-spacer {
    display: inline-block;
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

  /* File styling */
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

  /* Truncate from the beginning (show end of path) */
  .file-name.truncate-start {
    direction: rtl;
    text-align: left;
  }

  /* Unicode bidi override to keep path segments in correct order */
  .file-name.truncate-start::before {
    content: '\200E'; /* Left-to-right mark */
  }

  /* Status icon as interactive span */
  .status-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    background: none;
    border: none;
    padding: 2px;
    margin: -2px;
    cursor: pointer;
    color: var(--text-muted);
    border-radius: 3px;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .status-icon:hover {
    background-color: var(--bg-hover);
    color: var(--status-added);
  }

  .status-icon:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: 0;
  }

  /* Icon swap on hover */
  .icon-default,
  .icon-hover {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .icon-hover {
    display: none;
  }

  .status-icon:hover .icon-default {
    display: none;
  }

  .status-icon:hover .icon-hover {
    display: flex;
  }

  /* Unreview hover icon uses muted color instead of green */
  .icon-hover-unreview {
    color: var(--text-muted);
  }

  /* Comment indicator - must not shrink, file path will truncate instead */
  .comment-indicator {
    display: flex;
    align-items: center;
    color: var(--text-muted);
    flex-shrink: 0;
    margin-left: auto;
    padding-left: 4px;
  }

  /* Comments section */

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
    padding-right: 32px; /* Space for delete button */
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

  /* Delete button for comments */
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
      color 0.1s,
      background-color 0.1s;
    z-index: 1;
  }

  .comment-item-container:hover .comment-delete-btn {
    opacity: 1;
  }

  .comment-delete-btn:hover {
    color: var(--status-deleted);
    background-color: var(--bg-primary);
  }

  /* AI comment styling */
  .ai-comment .comment-icon {
    color: var(--text-accent);
  }

  .ai-comment.category-warning .comment-icon {
    color: var(--orange-9);
  }

  .ai-comment.category-suggestion .comment-icon {
    color: var(--green-9);
  }

  .ai-comment.category-explanation .comment-icon {
    color: var(--blue-9);
  }

  .ai-comment.category-context .comment-icon {
    color: var(--text-faint);
  }

  /* Reference files section */
  .add-file-btn {
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

  .add-file-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .add-file-btn:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: -2px;
  }

  .reference-section {
    opacity: 0.85;
  }

  .reference-item {
    position: relative;
  }

  .reference-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .remove-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-faint);
    cursor: pointer;
    opacity: 0;
    transition:
      opacity 0.1s,
      background-color 0.1s,
      color 0.1s;
    margin-left: auto;
    flex-shrink: 0;
  }

  .reference-item:hover .remove-btn {
    opacity: 1;
  }

  .remove-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  /* Agent section header - inside file-list, no extra margin needed */
  .agent-header {
    margin-bottom: 0;
  }

  /* Commit button in section header */
  .commit-btn {
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

  .commit-btn:hover:not(:disabled) {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .commit-btn:disabled,
  .commit-btn.disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .commit-btn:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: -2px;
  }

  /* Copy comments button in comments section header */
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

  .copy-btn:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: -2px;
  }

  /* Delete all comments button in comments section header */
  .delete-all-btn {
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

  .delete-all-btn:hover {
    background-color: var(--bg-hover);
    color: var(--status-deleted);
  }

  .delete-all-btn:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: -2px;
  }

  /* View AI Analysis button in agent section header */
  .view-ai-btn {
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

  .view-ai-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .view-ai-btn.active {
    background-color: var(--bg-primary);
    color: var(--text-accent);
  }

  .view-ai-btn:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: -2px;
  }

  /* AI Analysis button in agent section header */
  .ai-btn {
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

  .ai-btn:hover:not(:disabled) {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .ai-btn:disabled,
  .ai-btn.disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .ai-btn:focus-visible {
    outline: 2px solid var(--text-accent);
    outline-offset: -2px;
  }

  .ai-icon {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .ai-icon.spinning {
    animation: spin 2s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  /* Search results */
  .search-results-container {
    display: flex;
    flex-direction: column;
    background-color: var(--bg-secondary);
    border-left: 2px solid var(--border-subtle);
    margin-left: 8px;
  }

  .show-more-btn,
  .show-less-btn {
    display: block;
    width: 100%;
    padding: 4px 12px;
    background: none;
    border: none;
    color: var(--text-faint);
    font-size: var(--size-xs);
    text-align: center;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .show-more-btn:hover,
  .show-less-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-muted);
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

  /* Error Modal */
  .error-modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .error-modal {
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 420px;
    max-width: 90vw;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .error-modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .error-modal-header h2 {
    margin: 0;
    font-size: var(--size-base);
    font-weight: 600;
    color: var(--ui-danger);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .error-modal-body {
    padding: 20px;
    overflow-y: auto;
  }

  .error-message {
    margin: 0;
    color: var(--text-secondary);
    font-size: var(--size-sm);
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .error-modal-footer {
    display: flex;
    justify-content: flex-end;
    padding: 12px 20px;
    border-top: 1px solid var(--border-subtle);
  }
</style>
