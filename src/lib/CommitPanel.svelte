<script lang="ts">
  import { onMount } from 'svelte';
  import { getGitStatus } from './services/git';

  let commitMessage = $state('');
  let amendCommit = $state(false);
  let currentBranch = $state<string | null>(null);

  onMount(() => {
    loadBranch();
  });

  async function loadBranch() {
    try {
      const status = await getGitStatus();
      currentBranch = status.branch;
    } catch (e) {
      console.error('Failed to get branch:', e);
    }
  }

  function handleCommit() {
    console.log('Committing:', commitMessage, 'Amend:', amendCommit);
    // TODO: Implement actual commit via Tauri command
  }

  function handleCommitAndPush() {
    console.log('Commit and push:', commitMessage);
    // TODO: Implement actual commit + push via Tauri command
  }
</script>

<div class="commit-panel-content">
  <div class="branch-info">
    <span class="branch-icon">⎇</span>
    <span class="branch-name">{currentBranch ?? 'loading...'}</span>
  </div>

  <div class="commit-form">
    <input
      type="text"
      class="commit-input"
      placeholder="Commit message"
      bind:value={commitMessage}
    />

    <div class="commit-options">
      <label class="amend-checkbox">
        <input type="checkbox" bind:checked={amendCommit} />
        <span>Amend</span>
      </label>
    </div>

    <div class="commit-actions">
      <button 
        class="btn btn-primary" 
        onclick={handleCommit}
        disabled={!commitMessage.trim()}
      >
        Commit
      </button>
      <button 
        class="btn btn-secondary"
        onclick={handleCommitAndPush}
        disabled={!commitMessage.trim()}
      >
        Commit and Push
      </button>
    </div>
  </div>
</div>

<style>
  .commit-panel-content {
    display: flex;
    flex-direction: column;
    height: 100%;
    padding: 12px 16px;
    box-sizing: border-box;
  }

  .branch-info {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 8px;
    font-size: 12px;
    color: #888;
  }

  .branch-icon {
    font-size: 14px;
  }

  .branch-name {
    color: #4fc1ff;
    font-weight: 500;
  }

  .commit-form {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: 1;
  }

  .commit-input {
    flex: 1;
    padding: 8px 12px;
    font-size: 13px;
    background-color: #3c3c3c;
    border: 1px solid #4d4d4d;
    border-radius: 4px;
    color: #d4d4d4;
    outline: none;
  }

  .commit-input:focus {
    border-color: #0e639c;
  }

  .commit-input::placeholder {
    color: #888;
  }

  .commit-options {
    display: flex;
    align-items: center;
  }

  .amend-checkbox {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: #888;
    cursor: pointer;
  }

  .amend-checkbox input {
    cursor: pointer;
  }

  .commit-actions {
    display: flex;
    gap: 8px;
  }

  .btn {
    padding: 8px 16px;
    font-size: 13px;
    border: none;
    border-radius: 4px;
    cursor: pointer;
    font-weight: 500;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-primary {
    background-color: #0e639c;
    color: white;
  }

  .btn-primary:hover:not(:disabled) {
    background-color: #1177bb;
  }

  .btn-secondary {
    background-color: #3c3c3c;
    color: #d4d4d4;
    border: 1px solid #4d4d4d;
  }

  .btn-secondary:hover:not(:disabled) {
    background-color: #4d4d4d;
  }
</style>
