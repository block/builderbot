<script lang="ts">
  import { fade, slide } from 'svelte/transition';
  import AlertTriangle from '@lucide/svelte/icons/alert-triangle';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import ParentBranchCommitsHover from './ParentBranchCommitsHover.svelte';
  import { Button, buttonVariants } from '$lib/components/ui/button';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { cn } from '$lib/components/utils';
  import type { ProjectRepo } from '../../types';

  interface Props {
    branchId?: string;
    branchName: string;
    repoLabel?: ProjectRepo | null;
    baseBranch?: string | null;
    parentAheadCount?: number;
    onRebase?: () => void;
    /** Rebase, then force push once the rebase succeeds. Offered from the menu
     *  joined to the Rebase button; shares its disabled state. */
    onRebaseAndForcePush?: () => void;
    /** Why the visible Rebase button can't run right now, shown as its tooltip.
     *  Callers withhold `onRebase` instead when the button shouldn't be offered
     *  at all. */
    rebaseDisabledReason?: string | null;
    warning?: string | null;
    refreshingGitState?: boolean;
    fetchError?: string | null;
  }

  let {
    branchId,
    branchName,
    repoLabel = null,
    baseBranch = null,
    parentAheadCount = 0,
    onRebase,
    onRebaseAndForcePush,
    rebaseDisabledReason = null,
    warning = null,
    refreshingGitState = false,
    fetchError = null,
  }: Props = $props();

  const capsuleTitle = $derived.by(() => {
    if (!baseBranch || parentAheadCount <= 0) return baseBranch ?? undefined;
    const behind = `${parentAheadCount} commit${parentAheadCount === 1 ? '' : 's'} behind`;
    return refreshingGitState
      ? `${baseBranch} · ${behind} (checking for updates…)`
      : `${baseBranch} · ${behind}`;
  });
</script>

{#snippet capsule()}
  <span class="branch-capsule" title={capsuleTitle}>
    {baseBranch}{#if parentAheadCount > 0}<span
        class="ahead-count"
        class:provisional={refreshingGitState}
        transition:fade={{ duration: 150 }}
      >
        +{parentAheadCount}</span
      >{/if}
  </span>
{/snippet}

{#snippet parentPill()}
  {#if baseBranch}
    {#if parentAheadCount > 0 && branchId}
      <ParentBranchCommitsHover {branchId} {baseBranch} count={parentAheadCount}>
        {@render capsule()}
      </ParentBranchCommitsHover>
    {:else}
      {@render capsule()}
    {/if}
    {#if parentAheadCount > 0 && onRebase}
      <span class="inline-flex" transition:slide={{ axis: 'x', duration: 150 }}>
        <span
          class="inline-flex"
          title={rebaseDisabledReason ??
            'Rebase onto parent. Merge commits are linearised: pure automatic merges are dropped, and hand edits made inside a merge are kept as their own commit.'}
        >
          <Button
            variant="outline"
            size="xs"
            disabled={!!rebaseDisabledReason}
            onclick={onRebase}
            class={cn('h-[22px]', onRebaseAndForcePush && 'rounded-r-none')}>Rebase</Button
          >
        </span>
        {#if onRebaseAndForcePush}
          <!-- Joined to the Rebase button: the chevron shares its outline and
               height, with the touching corners squared and the border
               overlapped so the pair reads as one control. The title sits on
               a wrapping span, as on the Rebase button, because browsers
               don't reliably show tooltips on a disabled element. -->
          <DropdownMenu.Root>
            <span class="inline-flex -ml-px" title={rebaseDisabledReason ?? 'More rebase options'}>
              <DropdownMenu.Trigger
                class={cn(
                  buttonVariants({ variant: 'outline', size: 'xs' }),
                  'h-[22px] w-[18px] rounded-l-none px-0'
                )}
                disabled={!!rebaseDisabledReason}
                aria-label="More rebase options"
              >
                <ChevronDown size={12} />
              </DropdownMenu.Trigger>
            </span>
            <DropdownMenu.Content align="end" sideOffset={4} class="min-w-[200px]">
              <DropdownMenu.Item
                title="Rebase onto parent, then force push the rewritten branch to origin once the rebase succeeds."
                onSelect={() => onRebaseAndForcePush()}
              >
                Rebase and force push
              </DropdownMenu.Item>
            </DropdownMenu.Content>
          </DropdownMenu.Root>
        {/if}
      </span>
    {/if}
  {/if}
{/snippet}

{#snippet refreshStatus()}
  {#if refreshingGitState}
    <Spinner size={10} />
  {:else if fetchError}
    <span class="fetch-error" title={fetchError} transition:slide={{ axis: 'x', duration: 150 }}>
      <AlertTriangle size={12} />
    </span>
  {/if}
{/snippet}

<div class="header-left">
  {#if repoLabel}
    <span class="repo-name"
      ><RepoLabel
        githubRepo={repoLabel.headRepo ?? repoLabel.githubRepo}
        subpath={repoLabel.subpath}
      /></span
    >
    <div class="header-meta">
      <span class="branch-capsule" title={branchName}>{branchName}</span>
      {#if baseBranch}
        <ChevronRight size={12} />
      {/if}
      {@render parentPill()}
      {#if warning}
        <span class="branch-warning" title={warning}>
          <AlertTriangle size={12} />
          <span>{warning}</span>
        </span>
      {/if}
      {@render refreshStatus()}
    </div>
  {:else}
    <span class="repo-name">{branchName}</span>
    {#if baseBranch || warning || refreshingGitState || fetchError}
      <div class="header-meta">
        {@render parentPill()}
        {#if warning}
          <span class="branch-warning" title={warning}>
            <AlertTriangle size={12} />
            <span>{warning}</span>
          </span>
        {/if}
        {@render refreshStatus()}
      </div>
    {/if}
  {/if}
</div>

<style>
  .header-left {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1 1 240px;
  }

  .repo-name {
    display: block;
    font-size: var(--size-md);
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: -0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .header-meta {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
    overflow: hidden;
    font-size: var(--size-xs);
  }

  .header-meta :global(svg) {
    flex-shrink: 0;
    color: var(--text-faint);
  }

  .branch-capsule {
    display: inline-block;
    padding: 2px 8px;
    border-radius: 999px;
    background: none;
    border: 1px solid var(--border-subtle);
    color: var(--text-muted);
    font-size: var(--size-xs);
    max-width: 240px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ahead-count {
    font-weight: 600;
    color: var(--ui-accent);
  }

  .ahead-count.provisional {
    opacity: 0.6;
  }

  .branch-warning {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    max-width: 180px;
    color: var(--ui-warning, var(--status-modified));
    overflow: hidden;
    white-space: nowrap;
  }

  .branch-warning span {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .fetch-error {
    display: inline-flex;
    align-items: center;
    color: var(--ui-warning, var(--status-modified));
  }
</style>
