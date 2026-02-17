<script lang="ts">
  interface Props {
    branchName: string;
    repoLabel?: string | null;
    secondaryLabel?: string | null;
  }

  let { branchName, repoLabel = null, secondaryLabel = null }: Props = $props();
</script>

<div class="header-left">
  <span class="branch-name">{branchName}</span>
  {#if repoLabel || secondaryLabel}
    <div class="header-meta">
      {#if repoLabel}
        <span class="repo-name" title={repoLabel}>{repoLabel}</span>
      {/if}
      {#if repoLabel && secondaryLabel}
        <span class="meta-separator" aria-hidden="true">•</span>
      {/if}
      {#if secondaryLabel}
        <span class="base-branch-name" title={secondaryLabel}>{secondaryLabel}</span>
      {/if}
    </div>
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

  .branch-name {
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
    gap: 6px;
    min-width: 0;
    font-size: var(--size-xs);
  }

  .base-branch-name {
    color: var(--text-faint);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-name {
    color: var(--text-muted);
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
