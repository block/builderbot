<script lang="ts">
  import { AlertTriangle, GitBranch } from 'lucide-svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import type { ProjectRepo } from '../../types';

  interface Props {
    branchName: string;
    repoLabel?: ProjectRepo | null;
    secondaryLabel?: string | null;
    warning?: string | null;
  }

  let { branchName, repoLabel = null, secondaryLabel = null, warning = null }: Props = $props();
</script>

<div class="header-left">
  {#if repoLabel}
    <span class="repo-name"
      ><RepoLabel
        githubRepo={repoLabel.headRepo ?? repoLabel.githubRepo}
        subpath={repoLabel.subpath}
      /></span
    >
    <div class="header-meta">
      <span class="branch-name">{branchName}</span>
      {#if warning}
        <span class="branch-warning" title={warning}>
          <AlertTriangle size={12} />
          <span>{warning}</span>
        </span>
      {/if}
      {#if secondaryLabel}
        <span class="meta-separator" aria-hidden="true">&middot;</span>
        <GitBranch size={12} />
        <span class="base-branch-name" title={secondaryLabel}>{secondaryLabel}</span>
      {/if}
    </div>
  {:else}
    <span class="repo-name">{branchName}</span>
    {#if secondaryLabel || warning}
      <div class="header-meta">
        {#if secondaryLabel}
          <span class="base-branch-name" title={secondaryLabel}>{secondaryLabel}</span>
        {/if}
        {#if warning}
          <span class="branch-warning" title={warning}>
            <AlertTriangle size={12} />
            <span>{warning}</span>
          </span>
        {/if}
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

  .branch-name {
    max-width: 200px;
    color: var(--text-muted);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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

  .base-branch-name {
    color: var(--text-faint);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta-separator {
    color: var(--text-faint);
    flex-shrink: 0;
  }
</style>
