<!--
  AddRepoModal.svelte - Modal for adding a repository to an existing project

  Wraps RepoConfigForm in a dialog overlay with an "Add Repository" action button.
-->
<script lang="ts">
  import { slide } from 'svelte/transition';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoConfigForm from './RepoConfigForm.svelte';
  import type { PullRequest } from '../../types';
  import type { RepoSelection } from '../../shared/githubUrl';

  interface Props {
    open: boolean;
    excludeRepos?: Set<string>;
    onAdded: (selection: RepoSelection) => void;
    onClose: () => void;
  }

  let { open, excludeRepos, onAdded, onClose }: Props = $props();

  let selectedRepo = $state<string | null>(null);
  let headRepo = $state<string | null>(null);
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

      if (excludeRepos?.has(`${selectedRepo}\x00${normalizedSubpath ?? ''}`)) {
        error = 'This repo + subpath is already in the project';
        saving = false;
        return;
      }

      onAdded({
        nameWithOwner: selectedRepo,
        branchName: normalizedBranch,
        subpath: normalizedSubpath,
        prNumber,
        defaultBranch: matchedPr?.baseRef ?? defaultBranch ?? undefined,
        headRepo: headRepo ?? undefined,
        prTitle: matchedPr?.title,
        prBody: matchedPr?.body,
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

  function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    handleAdd();
  }
</script>

<Dialog.Root {open} onOpenChange={(v) => !v && onClose()}>
  <Dialog.Content class="sm:max-w-[460px] gap-3.5">
    <Dialog.Header>
      <Dialog.Title>Add Repository</Dialog.Title>
    </Dialog.Header>

    <form class="flex flex-col gap-3.5" onsubmit={handleSubmit}>
      <RepoConfigForm
        bind:selectedRepo
        bind:headRepo
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

      <div class="flex justify-end gap-2">
        <Button type="submit" variant="outline" class="w-full" disabled={!selectedRepo || saving}>
          {#if saving}
            <span class="inline-flex items-center gap-1.5">
              <Spinner size={14} />
              <span>Adding...</span>
            </span>
          {:else}
            Add
          {/if}
        </Button>
      </div>
    </form>
  </Dialog.Content>
</Dialog.Root>

<style>
  .error-message {
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }
</style>
