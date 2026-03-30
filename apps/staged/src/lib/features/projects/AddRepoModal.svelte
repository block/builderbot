<!--
  AddRepoModal.svelte - Modal for adding a repository to an existing project

  Wraps RepoConfigForm in a dialog overlay with an "Add Repository" action button.
-->
<script lang="ts">
  import { X } from 'lucide-svelte';
  import { slide } from 'svelte/transition';
  import { createBackdropDismissHandlers } from '../../shared/backdropDismiss';
  import FormButton from '../../shared/FormButton.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoConfigForm from './RepoConfigForm.svelte';
  import type { PullRequest } from '../../types';
  import type { RepoSelection } from '../../shared/githubUrl';

  interface Props {
    excludeRepos?: Set<string>;
    onAdded: (selection: RepoSelection) => void;
    onClose: () => void;
  }

  let { excludeRepos, onAdded, onClose }: Props = $props();
  const backdropDismiss = createBackdropDismissHandlers({ onDismiss: () => onClose() });

  let selectedRepo = $state<string | null>(null);
  let subpath = $state('');
  let branchName = $state('');
  let isNewBranch = $state(false);
  let matchedPr = $state<PullRequest | null>(null);
  let defaultBranch = $state<string | null>(null);
  let saving = $state(false);
  let error = $state<string | null>(null);
  let repoConfigApi = $state<
    | {
        waitForSubpathValidation: () => Promise<boolean>;
        selectRepo: (selection: RepoSelection) => void;
      }
    | undefined
  >(undefined);

  // Clear error when user edits the subpath
  $effect(() => {
    subpath;
    error = null;
  });

  async function handleAdd() {
    if (!selectedRepo || saving) return;

    saving = true;
    error = null;

    try {
      // Validate subpath if non-empty
      if (subpath.trim() && repoConfigApi) {
        const isValid = await repoConfigApi.waitForSubpathValidation();
        if (!isValid) {
          error = 'Invalid path in repo';
          saving = false;
          return;
        }
      }

      const normalizedSubpath = subpath.trim().replace(/^\/+|\/+$/g, '') || undefined;
      const normalizedBranch = branchName.trim() || undefined;
      const prNumber = matchedPr?.number ?? undefined;

      onAdded({
        nameWithOwner: selectedRepo,
        branchName: normalizedBranch,
        subpath: normalizedSubpath,
        prNumber,
        defaultBranch,
      });
      onClose();
    } catch (e) {
      if (typeof e === 'string') {
        error = e;
      } else if (e instanceof Error) {
        error = e.message;
      } else {
        error = String(e);
      }
      saving = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
      return;
    }

    if (e.key === 'Enter') {
      const target = e.target as HTMLElement;
      if (target.closest('.repo-search-wrapper')) return;
      if (target.closest('.subpath-input-wrapper')) return;
      if (target.closest('.branch-picker-wrapper')) return;
      e.preventDefault();
      handleAdd();
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
  onpointerdown={backdropDismiss.handlePointerDown}
  onclick={backdropDismiss.handleClick}
  onkeydown={(e) => e.key === 'Escape' && onClose()}
>
  <div class="modal">
    <div class="modal-header">
      <h2>Add Repository</h2>
      <button class="close-button" onclick={onClose}>
        <X size={18} />
      </button>
    </div>

    <div class="modal-body">
      <div class="add-repo-form">
        <RepoConfigForm
          bind:selectedRepo
          bind:subpath
          bind:branchName
          bind:isNewBranch
          bind:matchedPr
          bind:defaultBranch
          bind:api={repoConfigApi}
          {excludeRepos}
          autofocus
        />

        {#if error}
          <div class="error-message" transition:slide={{ duration: 150 }}>{error}</div>
        {/if}

        <div class="actions">
          <FormButton
            variant="primary"
            class="full-width-btn"
            onclick={handleAdd}
            disabled={!selectedRepo || saving}
          >
            {#if saving}
              <span class="button-content">
                <Spinner size={14} />
                <span>Adding...</span>
              </span>
            {:else}
              Add
            {/if}
          </FormButton>
        </div>
      </div>
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
    padding-top: 12vh;
    z-index: 1000;
  }

  .modal {
    width: 460px;
    max-width: 90vw;
    background-color: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    display: flex;
    flex-direction: column;
  }

  .modal-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    border-bottom: none;
    border-radius: 12px 12px 0 0;
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
  }

  .add-repo-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .error-message {
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  :global(.full-width-btn) {
    width: 100%;
  }

  .button-content {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
</style>
