<script lang="ts">
  import { AlertTriangle, ChevronRight } from 'lucide-svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import type { ProjectRepo } from '../../types';

  interface Props {
    branchName: string;
    repoLabel?: ProjectRepo | null;
    baseBranch?: string | null;
    parentAheadCount?: number;
    onRebase?: () => void;
    rebaseDisabled?: boolean;
    warning?: string | null;
  }

  let {
    branchName,
    repoLabel = null,
    baseBranch = null,
    parentAheadCount = 0,
    onRebase,
    rebaseDisabled = false,
    warning = null,
  }: Props = $props();
</script>

{#snippet parentPill()}
  {#if baseBranch}
    <span class="branch-capsule" title={baseBranch}>
      {baseBranch}{#if parentAheadCount > 0}<span class="ahead-count">
          +{parentAheadCount}</span
        >{/if}
    </span>
    {#if parentAheadCount > 0 && onRebase}
      <button
        class="rebase-btn"
        disabled={rebaseDisabled}
        title={rebaseDisabled ? 'Rebase unavailable' : 'Rebase onto parent'}
        onclick={onRebase}>Rebase</button
      >
    {/if}
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
    </div>
  {:else}
    <span class="repo-name">{branchName}</span>
    {#if baseBranch || warning}
      <div class="header-meta">
        {@render parentPill()}
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

  .rebase-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 22px;
    padding: 0 8px;
    font-size: var(--size-xs);
    font-weight: 500;
    color: var(--text-muted);
    background: none;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    cursor: pointer;
    white-space: nowrap;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s;
  }

  .rebase-btn:hover:not(:disabled) {
    border-color: var(--border-muted);
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .rebase-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
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
</style>
