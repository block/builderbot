<script lang="ts">
  import { fade, slide } from 'svelte/transition';
  import AlertTriangle from '@lucide/svelte/icons/alert-triangle';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import ParentBranchCommitsHover from './ParentBranchCommitsHover.svelte';
  import { Button } from '$lib/components/ui/button';
  import * as Tooltip from '$lib/components/ui/tooltip';
  import type { ProjectRepo } from '../../types';

  interface Props {
    branchId?: string;
    branchName: string;
    repoLabel?: ProjectRepo | null;
    baseBranch?: string | null;
    parentAheadCount?: number;
    onRebase?: () => void;
    rebaseDisabled?: boolean;
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
    rebaseDisabled = false,
    warning = null,
    refreshingGitState = false,
    fetchError = null,
  }: Props = $props();
</script>

{#snippet capsule()}
  <Tooltip.Root>
    <Tooltip.Trigger>
      {#snippet child({ props })}
        <span class="branch-capsule" {...props}>
          {baseBranch}{#if parentAheadCount > 0}<span
              class="ahead-count"
              transition:fade={{ duration: 150 }}
            >
              +{parentAheadCount}</span
            >{/if}
        </span>
      {/snippet}
    </Tooltip.Trigger>
    <Tooltip.Content>{baseBranch}</Tooltip.Content>
  </Tooltip.Root>
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
      <Tooltip.Root>
        <Tooltip.Trigger>
          {#snippet child({ props })}
            <span {...props} class="inline-flex" transition:slide={{ axis: 'x', duration: 150 }}>
              <Button
                variant="outline"
                size="xs"
                disabled={rebaseDisabled}
                onclick={onRebase}
                class="h-[22px]">Rebase</Button
              >
            </span>
          {/snippet}
        </Tooltip.Trigger>
        <Tooltip.Content>
          {rebaseDisabled ? 'Rebase unavailable' : 'Rebase onto parent'}
        </Tooltip.Content>
      </Tooltip.Root>
    {/if}
  {/if}
{/snippet}

{#snippet refreshStatus()}
  {#if refreshingGitState}
    <Spinner size={10} />
  {:else if fetchError}
    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <span class="fetch-error" {...props} transition:slide={{ axis: 'x', duration: 150 }}>
            <AlertTriangle size={12} />
          </span>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>{fetchError}</Tooltip.Content>
    </Tooltip.Root>
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
      <Tooltip.Root>
        <Tooltip.Trigger>
          {#snippet child({ props })}
            <span class="branch-capsule" {...props}>{branchName}</span>
          {/snippet}
        </Tooltip.Trigger>
        <Tooltip.Content>{branchName}</Tooltip.Content>
      </Tooltip.Root>
      {#if baseBranch}
        <ChevronRight size={12} />
      {/if}
      {@render parentPill()}
      {#if warning}
        <Tooltip.Root>
          <Tooltip.Trigger>
            {#snippet child({ props })}
              <span class="branch-warning" {...props}>
                <AlertTriangle size={12} />
                <span>{warning}</span>
              </span>
            {/snippet}
          </Tooltip.Trigger>
          <Tooltip.Content>{warning}</Tooltip.Content>
        </Tooltip.Root>
      {/if}
      {@render refreshStatus()}
    </div>
  {:else}
    <span class="repo-name">{branchName}</span>
    {#if baseBranch || warning || refreshingGitState || fetchError}
      <div class="header-meta">
        {@render parentPill()}
        {#if warning}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <span class="branch-warning" {...props}>
                  <AlertTriangle size={12} />
                  <span>{warning}</span>
                </span>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>{warning}</Tooltip.Content>
          </Tooltip.Root>
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
    flex: 1;
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
