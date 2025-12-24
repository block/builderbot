<script lang="ts">
  import Sidebar, { type FileCategory } from './lib/Sidebar.svelte';
  import DiffViewer from './lib/DiffViewer.svelte';
  import CommitPanel from './lib/CommitPanel.svelte';
  import {
    getFileDiff,
    getUntrackedFileDiff,
    stageFile,
    unstageFile,
    discardFile,
  } from './lib/services/git';
  import { ask } from '@tauri-apps/plugin-dialog';
  import type { FileDiff } from './lib/types';

  let selectedFile: string | null = $state(null);
  let selectedCategory: FileCategory | null = $state(null);
  let currentDiff: FileDiff | null = $state(null);
  let diffLoading = $state(false);
  let diffError: string | null = $state(null);
  let sidebarRef: Sidebar | null = $state(null);
  let commitPanelRef: CommitPanel | null = $state(null);

  async function handleFileSelect(path: string, category: FileCategory) {
    selectedFile = path;
    selectedCategory = category;
    await loadDiff(path, category);
  }

  async function loadDiff(path: string, category: FileCategory) {
    diffLoading = true;
    diffError = null;
    currentDiff = null;

    try {
      if (category === 'untracked') {
        currentDiff = await getUntrackedFileDiff(path);
      } else {
        currentDiff = await getFileDiff(path, category === 'staged');
      }
    } catch (e) {
      diffError = e instanceof Error ? e.message : String(e);
      console.error('Failed to load diff:', e);
    } finally {
      diffLoading = false;
    }
  }

  async function handleStageFile() {
    if (!selectedFile || !selectedCategory) return;

    const filePath = selectedFile;
    const wasStaged = selectedCategory === 'staged';

    try {
      if (wasStaged) {
        // Already staged - unstage it
        await unstageFile(filePath);
      } else {
        // Stage the file
        await stageFile(filePath);
      }

      // Refresh sidebar
      await sidebarRef?.loadStatus();

      // Follow the file to its new category and reload the diff
      const newCategory: FileCategory = wasStaged ? 'unstaged' : 'staged';
      selectedFile = filePath;
      selectedCategory = newCategory;
      await loadDiff(filePath, newCategory);
    } catch (e) {
      console.error('Failed to stage/unstage file:', e);
      diffError = e instanceof Error ? e.message : String(e);
    }
  }

  async function handleDiscardFile() {
    if (!selectedFile || !selectedCategory) return;

    // Use Tauri's native dialog for confirmation
    const confirmed = await ask(`Discard all changes to ${selectedFile}? This cannot be undone.`, {
      title: 'Discard Changes',
      kind: 'warning',
      okLabel: 'Discard',
      cancelLabel: 'Cancel',
    });
    if (!confirmed) return;

    try {
      await discardFile(selectedFile);
      // Refresh sidebar and clear diff
      await sidebarRef?.loadStatus();
      currentDiff = null;
      selectedFile = null;
      selectedCategory = null;
    } catch (e) {
      console.error('Failed to discard file:', e);
      diffError = e instanceof Error ? e.message : String(e);
    }
  }

  // Determine button labels based on current state
  function getStageButtonLabel(): string {
    if (selectedCategory === 'staged') return 'Unstage';
    return 'Stage';
  }

  async function handleCommitComplete() {
    // Refresh sidebar and commit panel after successful commit
    await sidebarRef?.loadStatus();
    commitPanelRef?.refresh();
    // Clear the diff view since staged files are now committed
    currentDiff = null;
    selectedFile = null;
    selectedCategory = null;
  }
</script>

<main>
  <div class="app-container">
    <aside class="sidebar">
      <Sidebar bind:this={sidebarRef} onFileSelect={handleFileSelect} {selectedFile} />
    </aside>
    <section class="main-content">
      {#if diffLoading}
        <div class="loading-state">Loading diff...</div>
      {:else if diffError}
        <div class="error-state">
          <p>Error loading diff:</p>
          <p class="error-message">{diffError}</p>
        </div>
      {:else}
        <DiffViewer
          diff={currentDiff}
          onStageFile={handleStageFile}
          onDiscardFile={selectedCategory !== 'staged' ? handleDiscardFile : undefined}
          stageButtonLabel={getStageButtonLabel()}
        />
      {/if}
    </section>
  </div>
  <footer class="commit-panel">
    <CommitPanel bind:this={commitPanelRef} onCommitComplete={handleCommitComplete} />
  </footer>
</main>

<style>
  :global(body) {
    margin: 0;
    padding: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
    background-color: #1e1e1e;
    color: #d4d4d4;
  }

  main {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }

  .app-container {
    display: flex;
    flex: 1;
    overflow: hidden;
  }

  .sidebar {
    width: 280px;
    min-width: 200px;
    background-color: #252526;
    border-right: 1px solid #3c3c3c;
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

  .loading-state,
  .error-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: #888;
    font-size: 14px;
  }

  .error-state {
    color: #f14c4c;
  }

  .error-message {
    font-family: monospace;
    font-size: 12px;
    color: #888;
    margin-top: 8px;
  }

  .commit-panel {
    height: 120px;
    min-height: 80px;
    background-color: #252526;
    border-top: 1px solid #3c3c3c;
  }
</style>
