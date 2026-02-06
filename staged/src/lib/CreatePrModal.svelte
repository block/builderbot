<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import { GitPullRequest, Loader2 } from 'lucide-svelte';
  import {
    getPrForBranch,
    pushBranch,
    createPullRequest,
    updatePullRequest,
    generatePrDescription,
    type PullRequestInfo,
    type CreatePrResult,
    type Branch,
    type CommitInfo,
  } from './services/branch';
  import { openUrl } from './services/window';

  export let branch: Branch;
  export let commits: CommitInfo[] = [];

  const dispatch = createEventDispatcher<{
    close: void;
    created: { url: string; number: number };
  }>();

  // Form state
  let title = '';
  let body = '';
  let isDraft = false;

  // Loading/error state
  let isLoading = true;
  let isPushing = false;
  let isCreating = false;
  let isGenerating = false;
  let error: string | null = null;

  // Existing PR (if any)
  let existingPr: PullRequestInfo | null = null;

  onMount(async () => {
    await checkExistingPr();

    if (!existingPr) {
      // Populate with simple defaults immediately
      populateDefaults();
      // Start AI generation in background (don't await)
      generateDescription();
    }
  });

  async function checkExistingPr() {
    isLoading = true;
    error = null;
    try {
      existingPr = await getPrForBranch(branch.repoPath, branch.branchName);
      if (existingPr) {
        // Pre-fill with existing PR data
        title = existingPr.title;
        body = existingPr.body;
        isDraft = existingPr.draft;
      }
    } catch (e) {
      // No PR exists, that's fine
      existingPr = null;
    } finally {
      isLoading = false;
    }
  }

  async function generateDescription() {
    isGenerating = true;

    try {
      // Use worktreePath for git operations - that's where the branch's commits live
      const result = await generatePrDescription(
        branch.worktreePath,
        branch.branchName,
        branch.baseBranch
      );
      title = result.title;
      body = result.body;
    } catch (e) {
      // AI generation failed, keep the simple defaults
      console.warn('AI PR description generation failed:', e);
    } finally {
      isGenerating = false;
    }
  }

  function populateDefaults() {
    if (existingPr) return; // Already populated from existing PR

    // Default title: derive from branch name (represents the overall work)
    // Branch names like "add-dark-mode" become "Add dark mode"
    title = branch.branchName.replace(/[-_]/g, ' ').replace(/^\w/, (c) => c.toUpperCase());

    // Build body from commits (listed oldest to newest for narrative flow)
    if (commits.length > 0) {
      // commits are in reverse chronological order (newest first), so reverse for the body
      const orderedCommits = [...commits].reverse();
      const commitList = orderedCommits.map((c) => `- ${c.subject}`).join('\n');
      body = `## Changes\n\n${commitList}`;
    }
  }

  async function handleSubmit() {
    if (!title.trim()) {
      error = 'Title is required';
      return;
    }

    error = null;

    try {
      // Step 1: Push the branch
      isPushing = true;
      await pushBranch(branch.repoPath, branch.branchName, false);
      isPushing = false;

      // Step 2: Create or update PR
      isCreating = true;

      if (existingPr) {
        // Update existing PR
        await updatePullRequest(branch.repoPath, existingPr.number, title.trim(), body.trim());
        dispatch('created', { url: existingPr.url, number: existingPr.number });
      } else {
        // Create new PR
        // Strip "origin/" prefix from baseBranch - it's stored as a remote-tracking ref
        // but GitHub expects just the branch name
        const targetBranch = branch.baseBranch.replace(/^origin\//, '');
        const result = await createPullRequest(
          branch.repoPath,
          branch.branchName,
          targetBranch,
          title.trim(),
          body.trim(),
          isDraft
        );
        dispatch('created', { url: result.url, number: result.number });
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      isPushing = false;
      isCreating = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      dispatch('close');
    } else if (e.key === 'Enter' && e.metaKey) {
      handleSubmit();
    }
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      dispatch('close');
    }
  }

  function viewExistingPr() {
    if (existingPr) {
      openUrl(existingPr.url);
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="modal-backdrop" on:click={handleBackdropClick}>
  <div class="modal">
    {#if isLoading}
      <div class="modal-content">
        <div class="icon-wrapper">
          <Loader2 size={24} class="spinner" />
        </div>
        <div class="text-content">
          <h2>Checking for existing PR...</h2>
        </div>
      </div>
    {:else}
      <form on:submit|preventDefault={handleSubmit}>
        <div class="modal-content">
          <div class="icon-wrapper">
            <GitPullRequest size={24} />
          </div>
          <div class="text-content">
            <h2>{existingPr ? 'Update Pull Request' : 'Create Pull Request'}</h2>
            <p class="branch-info">
              {branch.branchName} → {branch.baseBranch.replace(/^origin\//, '')}
              {#if commits.length > 0}
                <span class="commit-count">
                  · {commits.length} commit{commits.length !== 1 ? 's' : ''}
                </span>
              {/if}
            </p>

            {#if existingPr}
              <div class="existing-pr-banner">
                PR #{existingPr.number} already exists.
                <button type="button" class="link-btn" on:click={viewExistingPr}>
                  View on GitHub
                </button>
              </div>
            {/if}

            <div class="form-group">
              <label for="pr-title">Title</label>
              <input
                id="pr-title"
                type="text"
                bind:value={title}
                placeholder="PR title"
                disabled={isPushing || isCreating}
              />
            </div>

            <div class="form-group">
              <label for="pr-body">Description</label>
              <textarea
                id="pr-body"
                bind:value={body}
                placeholder="Describe your changes..."
                rows="6"
                disabled={isPushing || isCreating}
              ></textarea>
            </div>

            {#if isGenerating}
              <div class="generating-banner">
                <Loader2 size={14} class="spinner" />
                <span>AI is generating a description...</span>
              </div>
            {/if}

            {#if !existingPr}
              <div class="form-group checkbox-group">
                <label>
                  <input
                    type="checkbox"
                    bind:checked={isDraft}
                    disabled={isPushing || isCreating}
                  />
                  Create as draft
                </label>
              </div>
            {/if}

            {#if error}
              <div class="error">{error}</div>
            {/if}
          </div>
        </div>

        <div class="modal-actions">
          <button
            type="button"
            class="btn btn-secondary"
            on:click={() => dispatch('close')}
            disabled={isPushing || isCreating}
          >
            Cancel
          </button>
          <button
            type="submit"
            class="btn btn-primary"
            disabled={isPushing || isCreating || !title.trim()}
          >
            {#if isPushing}
              Pushing...
            {:else if isCreating}
              {existingPr ? 'Updating...' : 'Creating...'}
            {:else}
              {existingPr ? 'Push & Update PR' : 'Push & Create PR'}
            {/if}
          </button>
        </div>
      </form>
    {/if}
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
    width: 480px;
    max-width: 90vw;
    max-height: 90vh;
    overflow: hidden;
  }

  .modal-content {
    display: flex;
    gap: 16px;
    padding: 24px;
  }

  .icon-wrapper {
    flex-shrink: 0;
    width: 40px;
    height: 40px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg-hover);
    border-radius: 10px;
    color: var(--ui-accent);
  }

  .text-content {
    flex: 1;
    min-width: 0;
  }

  .text-content h2 {
    margin: 0 0 4px 0;
    font-size: var(--size-base);
    font-weight: 600;
    color: var(--text-primary);
  }

  .branch-info {
    margin: 0 0 16px 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
    font-family: var(--font-mono);
  }

  .commit-count {
    color: var(--text-faint);
  }

  .existing-pr-banner {
    padding: 10px 12px;
    background: var(--bg-hover);
    border-radius: 6px;
    margin-bottom: 16px;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .generating-banner {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    background: var(--bg-hover);
    border-radius: 6px;
    margin-bottom: 12px;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .link-btn {
    background: none;
    border: none;
    color: var(--ui-accent);
    cursor: pointer;
    padding: 0;
    font-size: inherit;
    text-decoration: underline;
  }

  .link-btn:hover {
    color: var(--ui-accent-hover);
  }

  .form-group {
    margin-bottom: 12px;
  }

  .form-group label {
    display: block;
    margin-bottom: 4px;
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-muted);
  }

  .form-group input[type='text'],
  .form-group textarea {
    width: 100%;
    padding: 8px 10px;
    background: var(--bg-primary);
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    box-sizing: border-box;
  }

  .form-group input[type='text']:focus,
  .form-group textarea:focus {
    outline: none;
    border-color: var(--ui-accent);
  }

  .form-group textarea {
    resize: vertical;
    min-height: 100px;
  }

  .checkbox-group label {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    font-size: var(--size-sm);
    color: var(--text-primary);
  }

  .checkbox-group input[type='checkbox'] {
    width: 14px;
    height: 14px;
    cursor: pointer;
  }

  .error {
    padding: 10px 12px;
    background: var(--ui-danger-bg);
    border-radius: 6px;
    color: var(--ui-danger);
    font-size: var(--size-sm);
    margin-top: 12px;
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 16px 24px;
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-primary);
  }

  .btn {
    padding: 8px 16px;
    border: none;
    border-radius: 6px;
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition:
      background-color 0.1s,
      opacity 0.1s;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-secondary {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-secondary:hover:not(:disabled) {
    background: var(--border-subtle);
  }

  .btn-primary {
    background: var(--ui-accent);
    color: var(--bg-primary);
  }

  .btn-primary:hover:not(:disabled) {
    background: var(--ui-accent-hover);
  }

  :global(.spinner) {
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
</style>
