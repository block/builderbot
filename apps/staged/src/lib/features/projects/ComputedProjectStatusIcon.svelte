<!--
  ComputedProjectStatusIcon.svelte — the icon for a project's computed status:
  its aggregate PR state, or its workspace state when remote. Used by
  ProjectStatusIcon when no status option is chosen, and by the status picker's
  Default row.
-->
<script lang="ts">
  import Cloud from '@lucide/svelte/icons/cloud';
  import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
  import GitPullRequestClosed from '@lucide/svelte/icons/git-pull-request-closed';
  import GitPullRequestDraft from '@lucide/svelte/icons/git-pull-request-draft';
  import Sprout from '@lucide/svelte/icons/sprout';
  import type { WorkspaceStatus } from '../../types';
  import type { ComputedProjectStatus } from './projectStatusDisplay';

  interface Props {
    status: ComputedProjectStatus;
    size?: number;
  }

  let { status, size = 14 }: Props = $props();

  function cloudColor(workspaceStatus: WorkspaceStatus | null): string {
    switch (workspaceStatus) {
      case 'running':
        return 'var(--ui-accent)';
      case 'starting':
        return 'var(--ui-info)';
      case 'error':
        return 'var(--ui-danger)';
      case 'stopped':
      case 'suspended':
      default:
        return 'var(--text-muted)';
    }
  }

  function colorStyle(color: string): string {
    return `color: ${color}`;
  }
</script>

{#if status.kind === 'cloud'}
  <Cloud {size} style={colorStyle(cloudColor(status.workspaceStatus))} />
{:else if status.kind === 'placeholder'}
  <span class="placeholder" style:width="{size}px" style:height="{size}px" aria-hidden="true"
  ></span>
{:else if status.prStatus === 'merged'}
  <GitPullRequest {size} style={colorStyle('var(--ui-success)')} />
{:else if status.prStatus === 'checks_failing'}
  <GitPullRequest {size} style={colorStyle('var(--ui-danger)')} />
{:else if status.prStatus === 'open'}
  <GitPullRequest {size} />
{:else if status.prStatus === 'closed'}
  <GitPullRequestClosed {size} />
{:else if status.prStatus === 'conflict'}
  <GitPullRequestClosed {size} style={colorStyle('var(--ui-danger)')} />
{:else if status.hasCodeChanges}
  <GitPullRequestDraft {size} style={colorStyle('var(--text-muted)')} />
{:else}
  <Sprout {size} style={colorStyle('var(--text-faint)')} />
{/if}

<style>
  .placeholder {
    display: inline-block;
    flex-shrink: 0;
  }
</style>
