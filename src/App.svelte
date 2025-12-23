<script lang="ts">
  import Sidebar, { type FileCategory } from './lib/Sidebar.svelte'
  import DiffViewer from './lib/DiffViewer.svelte'
  import CommitPanel from './lib/CommitPanel.svelte'
  import { getFileDiff, getUntrackedFileDiff } from './lib/services/git'
  import type { FileDiff } from './lib/types'

  let selectedFile: string | null = $state(null);
  let selectedCategory: FileCategory | null = $state(null);
  let currentDiff: FileDiff | null = $state(null);
  let diffLoading = $state(false);
  let diffError: string | null = $state(null);

  async function handleFileSelect(path: string, category: FileCategory) {
    selectedFile = path;
    selectedCategory = category;
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
</script>

<main>
  <div class="app-container">
    <aside class="sidebar">
      <Sidebar 
        onFileSelect={handleFileSelect}
        selectedFile={selectedFile}
      />
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
        <DiffViewer diff={currentDiff} />
      {/if}
    </section>
  </div>
  <footer class="commit-panel">
    <CommitPanel />
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

  .loading-state, .error-state {
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
