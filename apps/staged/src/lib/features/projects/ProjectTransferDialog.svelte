<!--
  ProjectTransferDialog.svelte — confirm moving a project between local
  worktrees and a Blox workspace.

  Mounted once in App.svelte and driven by projectActions.pendingTransfer, so
  every transfer entry point (sidebar and landing-grid context menus,
  ProjectHome's top bar) shares one flow. A Dialog rather than an AlertDialog:
  it opens at once and shows the preflight loading, then one row per branch
  with what it carries. Blockers disable Transfer and say why. When a repo has
  uncommitted changes, or may have (a workspace the transfer resumes first),
  a checkbox (defaulting to the last answer) chooses whether they move or
  stay behind in the backup. When a repo has ignored files outside ignored
  folders (`.env`), a second, independent checkbox (also defaulting to the
  last answer, off at first) chooses whether those move too; ignored files
  over the cap a transfer carries disable it and say why.
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
  import {
    asksAboutIgnored,
    asksAboutUncommitted,
    ignoredTransferBlocked,
    issueMessage,
    transferActionLabel,
    uncommittedOnlyPossible,
  } from './projectTransfer';

  const dialogId = ++dialogCounter;
  const checkboxId = `project-transfer-uncommitted-${dialogId}`;
  const ignoredCheckboxId = `project-transfer-ignored-${dialogId}`;

  let pending = $derived(projectActions.pendingTransfer);
  let preflight = $derived(pending?.status === 'ready' ? pending.preflight : null);
  let title = $derived(pending ? transferActionLabel(pending.direction) : '');
  let repoBlocked = $derived(preflight?.repos.some((repo) => repo.blockers.length > 0) ?? false);
  let asksUncommitted = $derived(preflight ? asksAboutUncommitted(preflight) : false);
  let uncommittedLabel = $derived(
    preflight && uncommittedOnlyPossible(preflight)
      ? 'Transfer uncommitted changes, if there are any'
      : 'Transfer uncommitted changes'
  );
  let asksIgnored = $derived(preflight ? asksAboutIgnored(preflight) : false);
  let ignoredBlocked = $derived(preflight ? ignoredTransferBlocked(preflight) : null);
  // A repo over the cap isn't backed up (its row says its ignored files are
  // deleted with the source), so the note doesn't promise it.
  let backupNote = $derived(
    !asksIgnored
      ? ''
      : ignoredBlocked !== null
        ? ', after ignored files under the size limit are backed up on this machine'
        : pending?.direction === 'toRemote'
          ? ', after their ignored files are backed up on this machine'
          : ', after its ignored files are backed up on this machine'
  );

  // Each open starts from the remembered answer; a change made in another
  // window while this one is open doesn't flip the box under the user.
  let includeUncommitted = $state(true);
  let includeIgnored = $state(false);
  let openedFor: string | null = null;
  $effect(() => {
    const projectId = pending?.project.id ?? null;
    if (projectId === openedFor) return;
    openedFor = projectId;
    if (projectId) {
      includeUncommitted = preferences.transferUncommittedChanges;
      includeIgnored = preferences.transferIgnoredFiles;
    }
  });
  /** What the transfer would actually do with ignored files, for the rows. */
  let carryIgnored = $derived(asksIgnored && ignoredBlocked === null && includeIgnored);

  function close() {
    projectActions.cancelPendingTransfer();
  }

  function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    if (!preflight?.canTransfer) return;
    void projectActions.confirmPendingTransfer({ includeUncommitted, includeIgnored });
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
          class="size-7 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground mobile:size-10 [&_svg]:!size-[18px]"
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
                <ProjectTransferRepoRow {row} includeIgnored={carryIgnored} />
              {/each}
            </ul>
          {/if}

          {#if asksUncommitted}
            <div class="option">
              <div class="option-row">
                <Checkbox
                  id={checkboxId}
                  class="data-unchecked:bg-[var(--bg-primary)] dark:data-unchecked:bg-[var(--bg-primary)]"
                  bind:checked={includeUncommitted}
                />
                <Label for={checkboxId} class="font-normal">{uncommittedLabel}</Label>
              </div>
              {#if !includeUncommitted}
                <p class="option-hint">
                  Uncommitted changes are left behind: they're kept only in a backup on this
                  machine, and each branch starts from its last commit.
                </p>
              {/if}
            </div>
          {/if}

          {#if asksIgnored}
            <div class="option">
              <div class="option-row">
                <Checkbox
                  id={ignoredCheckboxId}
                  class="data-unchecked:bg-[var(--bg-primary)] dark:data-unchecked:bg-[var(--bg-primary)]"
                  checked={ignoredBlocked === null && includeIgnored}
                  onCheckedChange={(checked) => (includeIgnored = checked)}
                  disabled={ignoredBlocked !== null}
                  aria-describedby="{ignoredCheckboxId}-hint"
                />
                <Label for={ignoredCheckboxId} class="font-normal">
                  Also transfer ignored files (such as <code>.env</code>)
                </Label>
              </div>
              <p id="{ignoredCheckboxId}-hint" class="option-note">
                {#if ignoredBlocked}
                  <span class="option-hint">{ignoredBlocked}</span>
                {/if}
                Ignored folders such as <code>node_modules</code> are never transferred.
              </p>
            </div>
          {/if}

          <p class="footnote">
            The old {pending.direction === 'toRemote' ? 'worktrees are' : 'workspace is'} removed once
            the transfer succeeds{backupNote}. Sessions keep their history; follow-ups start a new
            agent conversation.
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
            class="gap-1.5 px-4 py-2 text-sm font-medium text-muted-foreground hover:text-foreground mobile:h-11 mobile:flex-1 mobile:justify-center"
            onclick={close}
          >
            Cancel
          </Button>
          <Button
            type="submit"
            variant="accent"
            class="gap-1.5 px-4 py-2 text-sm mobile:h-11 mobile:flex-1 mobile:justify-center"
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

  .option {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .option-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .option-hint {
    margin: 0;
    color: var(--status-modified);
    font-size: var(--size-xs);
  }

  .option-note {
    margin: 0;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .option-note .option-hint {
    display: block;
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

  /* The dialog's last row; clear of the home indicator in a full-screen
     dialog (`--dialog-safe-bottom`, see dialog-content). */
  .form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
    padding-bottom: var(--dialog-safe-bottom, 0px);
    flex-shrink: 0;
  }

  /* The mobile layout (MOBILE_LAYOUT_QUERY, viewport.svelte.ts). */
  @media (max-width: 768px),
    (pointer: coarse) and (max-device-width: 500px),
    (pointer: coarse) and (max-device-height: 500px) {
    .modal-header {
      padding: 12px 16px;
    }

    .modal-body {
      padding: 16px;
    }
  }
</style>
