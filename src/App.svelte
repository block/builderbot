<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import Sidebar from './lib/Sidebar.svelte';
  import DiffViewer from './lib/DiffViewer.svelte';
  import TopBar from './lib/TopBar.svelte';
  import { getRefs } from './lib/services/git';
  import type { GitRef, DiffSpec } from './lib/types';
  import {
    subscribeToFileChanges,
    startWatching,
    stopWatching,
    type Unsubscribe,
  } from './lib/services/statusEvents';
  import {
    preferences,
    loadSavedSize,
    loadSavedSyntaxTheme,
    handlePreferenceKeydown,
  } from './lib/stores/preferences.svelte';
  import {
    WORKDIR,
    diffSelection,
    selectDiffSpec,
    selectCustomDiff,
    initDiffSelection,
    setDefaultBranch,
  } from './lib/stores/diffSelection.svelte';
  import {
    diffState,
    getCurrentDiff,
    loadDiffs,
    refreshDiffs,
    selectFile,
    resetState,
  } from './lib/stores/diffState.svelte';
  import { loadComments, setCurrentPath } from './lib/stores/comments.svelte';
  import { repoState, initRepoState } from './lib/stores/repoState.svelte';

  // UI State
  let sidebarRef: Sidebar | null = $state(null);
  let unsubscribe: Unsubscribe | null = null;

  // Diff Loading
  async function loadAllDiffs() {
    await loadDiffs(
      diffSelection.spec.base,
      diffSelection.spec.head,
      repoState.currentPath ?? undefined
    );
    await loadComments(diffSelection.spec.base, diffSelection.spec.head);
    sidebarRef?.setDiffs(diffState.diffs);
  }

  // Update comments store when selected file changes
  $effect(() => {
    const path = currentDiff?.after?.path ?? currentDiff?.before?.path ?? null;
    setCurrentPath(path);
  });

  async function handleFilesChanged() {
    if (diffSelection.spec.head !== WORKDIR) return;
    // Use refreshDiffs to avoid loading flicker - keeps content visible during fetch
    await refreshDiffs(
      diffSelection.spec.base,
      diffSelection.spec.head,
      repoState.currentPath ?? undefined
    );
    // Reload comments - they may have changed after a commit
    await loadComments(diffSelection.spec.base, diffSelection.spec.head);
    sidebarRef?.setDiffs(diffState.diffs);
  }

  // Preset selection
  async function handleDiffSelect(spec: DiffSpec) {
    resetState();
    await selectDiffSpec(spec);
    await loadAllDiffs();
  }

  // Custom diff selection
  async function handleCustomDiff(base: string, head: string) {
    resetState();
    await selectCustomDiff(base, head);
    await loadAllDiffs();
  }

  // Repo change - reload everything
  async function handleRepoChange() {
    // Stop watching old repo
    await stopWatching().catch(() => {});
    unsubscribe?.();

    // Reset state
    resetState();

    if (repoState.currentPath && !repoState.error) {
      // Load refs and detect default branch for new repo
      try {
        const refs = await getRefs(repoState.currentPath);
        const defaultBranch = detectDefaultBranch(refs);
        setDefaultBranch(defaultBranch);
      } catch (e) {
        console.error('Failed to load refs:', e);
      }

      // Re-init diff selection and load diffs
      await initDiffSelection();
      await loadAllDiffs();

      // Start watching new repo
      try {
        await startWatching(repoState.currentPath);
        unsubscribe = await subscribeToFileChanges(handleFilesChanged);
      } catch (e) {
        console.error('Failed to start watcher:', e);
      }
    }
  }

  /**
   * Detect the default branch (main, master, etc.) from available refs.
   */
  function detectDefaultBranch(refs: GitRef[]): string {
    const branchNames = refs.filter((r) => r.ref_type === 'branch').map((r) => r.name);

    // Check common default branch names in order of preference
    const candidates = ['main', 'master', 'develop', 'trunk'];
    for (const name of candidates) {
      if (branchNames.includes(name)) {
        return name;
      }
    }

    // Fallback to first branch, or 'main' if no branches
    return branchNames[0] ?? 'main';
  }

  let currentDiff = $derived(getCurrentDiff());

  // Lifecycle
  onMount(() => {
    loadSavedSize();
    window.addEventListener('keydown', handlePreferenceKeydown);

    (async () => {
      await loadSavedSyntaxTheme();

      // Initialize repo state (loads recent repos, tries current directory)
      const hasRepo = await initRepoState();

      if (hasRepo && repoState.currentPath) {
        // Load refs for autocomplete and detect default branch
        try {
          const refs = await getRefs(repoState.currentPath);
          const defaultBranch = detectDefaultBranch(refs);
          setDefaultBranch(defaultBranch);
        } catch (e) {
          console.error('Failed to load refs:', e);
        }

        await initDiffSelection();
        await loadAllDiffs();

        // Start file watcher
        try {
          await startWatching(repoState.currentPath);
          unsubscribe = await subscribeToFileChanges(handleFilesChanged);
        } catch (e) {
          console.error('Failed to start watcher:', e);
        }
      }
    })();
  });

  onDestroy(() => {
    window.removeEventListener('keydown', handlePreferenceKeydown);
    unsubscribe?.();
    stopWatching().catch(() => {});
  });
</script>

<main>
  <TopBar
    onDiffSelect={handleDiffSelect}
    onCustomDiff={handleCustomDiff}
    onRepoChange={handleRepoChange}
  />

  <div class="app-container">
    <section class="main-content">
      {#if !repoState.currentPath || repoState.error}
        <div class="empty-state">
          <p>{repoState.error ?? 'No repository selected'}</p>
          <p class="empty-hint">Use the repository selector to open a git repository</p>
        </div>
      {:else if diffState.loading}
        <div class="loading-state">
          <p>Loading...</p>
        </div>
      {:else if diffState.error}
        <div class="error-state">
          <p>Error loading diff:</p>
          <p class="error-message">{diffState.error}</p>
        </div>
      {:else}
        <DiffViewer
          diff={currentDiff}
          diffBase={diffSelection.spec.base}
          diffHead={diffSelection.spec.head}
          sizeBase={preferences.sizeBase}
          syntaxThemeVersion={preferences.syntaxThemeVersion}
        />
      {/if}
    </section>
    <aside class="sidebar">
      <Sidebar
        bind:this={sidebarRef}
        onFileSelect={selectFile}
        selectedFile={diffState.selectedFile}
        diffBase={diffSelection.spec.base}
        diffHead={diffSelection.spec.head}
      />
    </aside>
  </div>
</main>

<style>
  :global(body) {
    margin: 0;
    padding: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
    background-color: var(--bg-chrome);
    color: var(--text-primary);
  }

  main {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
    background-color: var(--bg-chrome);
  }

  .app-container {
    display: flex;
    flex: 1;
    overflow: hidden;
    padding: 0 8px 8px 8px;
    gap: 8px;
  }

  .sidebar {
    width: 260px;
    min-width: 180px;
    background-color: transparent;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .main-content {
    flex: 1;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .loading-state {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
    font-size: var(--size-lg);
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
    font-size: var(--size-lg);
  }

  .empty-hint {
    font-size: var(--size-sm);
    color: var(--text-faint);
    margin-top: 8px;
  }

  .error-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--status-deleted);
    font-size: var(--size-lg);
  }

  .error-message {
    font-family: monospace;
    font-size: var(--size-sm);
    color: var(--text-muted);
    margin-top: 8px;
  }
</style>
