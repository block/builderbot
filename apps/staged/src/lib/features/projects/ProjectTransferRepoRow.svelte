<!--
  ProjectTransferRepoRow.svelte — one branch in the transfer dialog: its repo
  and branch, what it carries (unpushed commits, uncommitted changes, by
  kind), the ignored files and folders that stay behind, and whatever blocks or warns
  about it. Blockers and warnings differ in colour and in a screen-reader
  prefix.
-->
<script lang="ts">
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import type { ProjectTransferRepoPreflight } from '../../types';
  import {
    ignoredSummary,
    issueMessage,
    plural,
    uncommittedBreakdown,
    uncommittedCount,
  } from './projectTransfer';

  interface Props {
    row: ProjectTransferRepoPreflight;
  }

  let { row }: Props = $props();

  let changes = $derived(uncommittedCount(row));
  let breakdown = $derived(uncommittedBreakdown(row));
  let ignored = $derived(ignoredSummary(row));
  let renamed = $derived(row.destBranchName !== row.branchName);
</script>

<li class="repo-row" class:blocked={row.blockers.length > 0}>
  <div class="repo-head">
    <span class="repo-name">
      <RepoLabel githubRepo={row.githubRepo} subpath={row.subpath} />
    </span>
    <span
      class="branch-name"
      title={renamed ? `${row.branchName} lands as ${row.destBranchName}` : row.branchName}
    >
      <GitBranch size={12} />
      <span class="branch-text">
        {row.branchName}{#if renamed}&nbsp;→ {row.destBranchName}{/if}
      </span>
    </span>
  </div>

  <div class="repo-stats">
    {#if row.state === 'nothingToCarry'}
      <span>Nothing to carry</span>
    {:else if row.state === 'unknown'}
      <span>Checked once the workspace resumes</span>
    {:else}
      <span class:emphasis={row.unpushedCommits > 0}>
        {plural(row.unpushedCommits, 'unpushed commit')}
      </span>
      <span class:emphasis={changes > 0}>
        {plural(changes, 'uncommitted change')}{#if breakdown}&nbsp;({breakdown}){/if}
      </span>
      {#if ignored}
        <span class="ignored">{ignored}</span>
      {/if}
    {/if}
  </div>

  {#if row.blockers.length > 0 || row.warnings.length > 0}
    <ul class="issues">
      {#each row.blockers as issue, i (`blocker-${i}`)}
        <li class="issue blocker">
          <span class="sr-only">Blocks transfer:</span>
          {issueMessage(issue)}
        </li>
      {/each}
      {#each row.warnings as issue, i (`warning-${i}`)}
        <li class="issue warning">
          <span class="sr-only">Warning:</span>
          {issueMessage(issue)}
        </li>
      {/each}
    </ul>
  {/if}
</li>

<style>
  .repo-row {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px 10px;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    background: var(--bg-primary);
    min-width: 0;
  }

  .repo-row.blocked {
    border-color: var(--ui-danger);
  }

  .repo-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    min-width: 0;
    font-size: var(--size-sm);
  }

  .repo-name {
    min-width: 0;
    overflow: hidden;
  }

  .branch-name {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    flex-shrink: 1;
    color: var(--branch-color);
    font-size: var(--size-xs);
  }

  .branch-name :global(svg) {
    flex-shrink: 0;
  }

  .branch-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-stats {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .repo-stats .emphasis {
    color: var(--text-primary);
  }

  .repo-stats .ignored {
    color: var(--status-modified);
  }

  .issues {
    margin: 2px 0 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: var(--size-xs);
  }

  .issue.blocker {
    color: var(--ui-danger);
  }

  .issue.warning {
    color: var(--status-modified);
  }
</style>
