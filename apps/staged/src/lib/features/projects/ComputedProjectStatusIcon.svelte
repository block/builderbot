<!--
  ComputedProjectStatusIcon.svelte — the icon for a project's computed status:
  its aggregate PR state. Used by ProjectStatusIcon when the project has no tag
  in the icon group, and by the icon group's Default row in the header pill
  and the context menu.
-->
<script lang="ts">
  import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
  import GitPullRequestClosed from '@lucide/svelte/icons/git-pull-request-closed';
  import GitPullRequestDraft from '@lucide/svelte/icons/git-pull-request-draft';
  import Sprout from '@lucide/svelte/icons/sprout';
  import type { ComputedProjectStatus } from './projectStatusDisplay';

  interface Props {
    status: ComputedProjectStatus;
    size?: number;
  }

  let { status, size = 14 }: Props = $props();

  function colorStyle(color: string): string {
    return `color: ${color}`;
  }
</script>

{#if status.kind === 'placeholder'}
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
