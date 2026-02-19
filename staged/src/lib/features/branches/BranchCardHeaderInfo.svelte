<script lang="ts">
  import { GitBranch } from 'lucide-svelte';

  interface Props {
    branchName: string;
    repoLabel?: string | null;
    secondaryLabel?: string | null;
  }

  let { branchName, repoLabel = null, secondaryLabel = null }: Props = $props();
</script>

<div class="header-left">
  {#if repoLabel}
    <span class="repo-name" title={repoLabel}>{repoLabel}</span>
    <div class="header-meta">
      <span class="branch-name">{branchName}</span>
      {#if secondaryLabel}
        <span class="meta-separator" aria-hidden="true">&middot;</span>
        <GitBranch size={12} />
        <span class="base-branch-name" title={secondaryLabel}>{secondaryLabel}</span>
      {/if}
    </div>
  {:else}
    <span class="repo-name">{branchName}</span>
    {#if secondaryLabel}
      <div class="header-meta">
        <span class="base-branch-name" title={secondaryLabel}>{secondaryLabel}</span>
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
    font-size: var(--size-xs);
  }

  .header-meta :global(svg) {
    flex-shrink: 0;
    color: var(--text-faint);
  }

  .branch-name {
    color: var(--text-muted);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
