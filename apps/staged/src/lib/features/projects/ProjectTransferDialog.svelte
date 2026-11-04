<!--
  ProjectTransferDialog.svelte — confirm moving a project between local
  worktrees and a Blox workspace.

  Mounted once in App.svelte and driven by projectActions.pendingTransfer, so
  every transfer entry point (sidebar and landing-grid context menus,
  ProjectHome's top bar) shares one flow. A Dialog rather than an AlertDialog:
  it opens at once and shows the preflight loading, then one row per branch
  with what it carries. Blockers disable Transfer and say why. When a repo has
  uncommitted changes, a checkbox (defaulting to the last answer) chooses
  whether they move or stay behind in the backup.
-->
<script lang="ts" module>
  let dialogCounter = 0;
</script>

<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Checkbox } from '$lib/components/ui/checkbox';
  import { Label } from '$lib/components/ui/label';
  import Spinner from '../../shared/Spinner.svelte';
  import { projectDisplayName } from '../../shared/utils';
  import { preferences } from '../settings/preferences.svelte';
  import { projectActions } from './projectActions.svelte';
  import ProjectTransferRepoRow from './ProjectTransferRepoRow.svelte';
  import { issueMessage, transferActionLabel } from './projectTransfer';

  const checkboxId = `project-transfer-uncommitted-${++dialogCounter}`;

  let pending = $derived(projectActions.pendingTransfer);
  let preflight = $derived(pending?.status === 'ready' ? pending.preflight : null);
  let title = $derived(pending ? transferActionLabel(pending.direction) : '');
  let repoBlocked = $derived(preflight?.repos.some((repo) => repo.blockers.length > 0) ?? false);

  // Each open starts from the remembered answer; a change made in another
  // window while this one is open doesn't flip the box under the user.
  let includeUncommitted = $state(true);
  let openedFor: string | null = null;
  $effect(() => {
    const projectId = pending?.project.id ?? null;
    if (projectId === openedFor) return;
    openedFor = projectId;
    if (projectId) includeUncommitted = preferences.transferUncommittedChanges;
  });

  function close() {
    projectActions.cancelPendingTransfer();
  }

  function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    if (!preflight?.canTransfer) return;
    void projectActions.confirmPendingTransfer({ includeUncommitted });
  }
</script>

<Dialog.Root open={pending !== null} onOpenChange={(open) => !open && close()}>
  <Dialog.Content
    class="sm:max-w-[520px] p-0 gap-0 overflow-hidden flex flex-col"
    showCloseButton={false}
  >
    {#if pending}
      <header class="modal-header">
        <Dialog.Title class="text-[var(--size-sm)] font-semibold text-foreground">
          {title}
        </Dialog.Title>
        <Button
          variant="ghost"
          size="icon"
          class="size-7 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground max-[768px]:size-10 [&_svg]:!size-[18px]"
          title="Close"
          aria-label="Close"
          onclick={close}
        >
          <X size={18} />
        </Button>
      </header>

      <form class="modal-body" onsubmit={handleSubmit}>
        <Dialog.Description class="text-[var(--size-sm)]">
          {#if pending.direction === 'toRemote'}
            Move “{projectDisplayName(pending.project)}” to a new Blox workspace. Each branch moves
            with its commits, including any that aren't pushed yet.
          {:else}
            Move “{projectDisplayName(pending.project)}” from
            {preflight?.workspaceName ? `workspace ${preflight.workspaceName}` : 'Blox'} to local worktrees.
            Each branch moves with its commits, including any that aren't pushed yet.
          {/if}
        </Dialog.Description>

        {#if pending.status === 'loading'}
          <p class="loading" role="status">
            <Spinner size={14} />
            <span>Checking what would move…</span>
          </p>
        {:else if pending.status === 'error'}
          <p class="transfer-error" role="alert">Couldn't check the project: {pending.error}</p>
        {:else if preflight}
          {#if preflight.warnings.length > 0}
            <ul class="project-warnings">
              {#each preflight.warnings as issue, i (i)}
                <li>{issueMessage(issue)}</li>
              {/each}
            </ul>
          {/if}

          {#if preflight.repos.length > 0}
            <ul class="repo-list" aria-label="Branches">
              {#each preflight.repos as row (row.branchId)}
                <ProjectTransferRepoRow {row} />
              {/each}
            </ul>
          {/if}

          {#if preflight.hasUncommittedChanges}
            <div class="uncommitted">
              <div class="uncommitted-row">
                <Checkbox
                  id={checkboxId}
                  class="data-unchecked:bg-[var(--bg-primary)] dark:data-unchecked:bg-[var(--bg-primary)]"
                  bind:checked={includeUncommitted}
                />
                <Label for={checkboxId} class="font-normal">Transfer uncommitted changes</Label>
              </div>
              {#if !includeUncommitted}
                <p class="uncommitted-hint">
                  Uncommitted changes are left behind: they're kept only in a backup on this
                  machine, and each branch starts from its last commit.
                </p>
              {/if}
            </div>
          {/if}

          <p class="footnote">
            Ignored files such as <code>.env</code> don't move, and the old
            {pending.direction === 'toRemote' ? 'worktrees are' : 'workspace is'} removed once the transfer
            succeeds. Sessions keep their history; follow-ups start a new agent conversation.
          </p>

          {#if !preflight.canTransfer}
            <div class="blockers" role="alert">
              <p class="blockers-title">Can't transfer yet</p>
              {#if preflight.blockers.length > 0}
                <ul>
                  {#each preflight.blockers as issue, i (i)}
                    <li>{issueMessage(issue)}</li>
                  {/each}
                </ul>
              {/if}
              {#if repoBlocked}
                <p>Resolve the issues marked on the branches above, then try again.</p>
              {/if}
            </div>
          {/if}
        {/if}

        <div class="form-actions">
          <Button
            type="button"
            variant="outline"
            class="gap-1.5 px-4 py-2 text-sm font-medium text-muted-foreground hover:text-foreground max-[768px]:h-11 max-[768px]:flex-1 max-[768px]:justify-center"
            onclick={close}
          >
            Cancel
          </Button>
          <Button
            type="submit"
            variant="accent"
            class="gap-1.5 px-4 py-2 text-sm max-[768px]:h-11 max-[768px]:flex-1 max-[768px]:justify-center"
            disabled={!preflight?.canTransfer}
          >
            {title}
          </Button>
        </div>
      </form>
    {/if}
  </Dialog.Content>
</Dialog.Root>

<style>
  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 10px 18px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .modal-body {
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .loading {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .transfer-error {
    margin: 0;
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }

  .project-warnings {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--status-modified);
    font-size: var(--size-xs);
  }

  .repo-list {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .uncommitted {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .uncommitted-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .uncommitted-hint {
    margin: 0;
    color: var(--status-modified);
    font-size: var(--size-xs);
  }

  .footnote {
    margin: 0;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .blockers {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--ui-danger-bg);
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }

  .blockers p {
    margin: 0;
  }

  .blockers-title {
    font-weight: 600;
  }

  .blockers ul {
    margin: 0;
    padding-left: 16px;
  }

  .form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
    flex-shrink: 0;
  }

  @media (max-width: 768px) {
    .modal-header {
      padding: 12px 16px;
    }

    .modal-body {
      padding: 16px;
    }
  }
</style>
