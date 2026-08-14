<!--
  ProjectRowContent.svelte — how a project introduces itself in a row: a state
  icon on the left (aggregate PR status, or cloud status for remote projects),
  the name, and a meta line of colored repo badges with any running activity —
  falling back to a repo count while badges haven't landed.

  Shared between the sidebar's project rows and pickers that list projects
  (MoveBranchDialog), so a project reads the same everywhere. Everything is
  derived from the shared stores; parents only say which project. Parents
  theme the meta line through --project-row-meta-color (the sidebar's active
  row brightens it).
-->
<script lang="ts">
  import Cloud from '@lucide/svelte/icons/cloud';
  import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
  import GitPullRequestClosed from '@lucide/svelte/icons/git-pull-request-closed';
  import GitPullRequestDraft from '@lucide/svelte/icons/git-pull-request-draft';
  import Sprout from '@lucide/svelte/icons/sprout';
  import type { Project, WorkspaceStatus } from '../../types';
  import {
    projectDisplayName,
    aggregateProjectPrStatus,
    projectHasCodeChanges,
    projectSubtitle,
    projectActivity,
  } from '../../shared/utils';
  import RepoBadge from '../../shared/RepoBadge.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import { getProjectStatus } from './projectStatus';

  interface Props {
    project: Project;
  }

  let { project }: Props = $props();

  let branches = $derived(projectsDataStore.branchesByProject.get(project.id) ?? []);
  let status = $derived(
    getProjectStatus(project.id, projectsDataStore.deletingProjectNames, branches)
  );
  let prStatus = $derived(aggregateProjectPrStatus(branches));
  let workspaceStatus = $derived(
    project.location === 'remote'
      ? (branches.find((b) => b.workspaceStatus)?.workspaceStatus ?? null)
      : null
  );
  let badges = $derived(
    (projectsDataStore.reposByProject.get(project.id) ?? [])
      .map((r) => repoBadgeStore.lookup(r.githubRepo, r.subpath))
      .filter((b): b is NonNullable<typeof b> => Boolean(b))
      .sort((a, b) => a.shortName.localeCompare(b.shortName))
  );
  let sessionTypes = $derived(projectStateStore.getRunningSessionTypes(project.id));
  let activity = $derived(projectActivity(sessionTypes, status.runActionPhase));
  let repoCount = $derived(
    projectsDataStore.repoCountsByProject.get(project.id) ?? (project.githubRepo ? 1 : 0)
  );

  function cloudStatusClass(status: WorkspaceStatus | null): string {
    switch (status) {
      case 'running':
        return 'cloud-running';
      case 'starting':
        return 'cloud-starting';
      case 'error':
        return 'cloud-error';
      case 'stopped':
      case 'suspended':
      default:
        return 'cloud-inactive';
    }
  }
</script>

<div class="row-main">
  {#if project.location === 'remote'}
    <Cloud size={14} class={cloudStatusClass(workspaceStatus)} />
  {:else if prStatus === 'merged'}
    <GitPullRequest size={14} class="pr-status-merged" />
  {:else if prStatus === 'checks_failing'}
    <GitPullRequest size={14} class="pr-status-checks-failing" />
  {:else if prStatus === 'open'}
    <GitPullRequest size={14} />
  {:else if prStatus === 'closed'}
    <GitPullRequestClosed size={14} />
  {:else if prStatus === 'conflict'}
    <GitPullRequestClosed size={14} class="pr-status-conflict" />
  {:else if projectHasCodeChanges(branches)}
    <GitPullRequestDraft size={14} class="pr-status-draft" />
  {:else}
    <Sprout size={14} class="pr-status-clean" />
  {/if}
  <div class="row-text">
    <span class="project-name">{projectDisplayName(project)}</span>
    <div class="row-meta">
      {#if badges.length > 0}
        <span class="badge-row">
          {#each badges as badge}
            <RepoBadge shortName={badge.shortName} hue={badge.hue} small />
          {/each}
        </span>
        {#if activity}
          <span class="activity-separator">&middot;</span>
          <span class="activity-text">{activity}</span>
        {/if}
      {:else}
        <span class="repo-count"
          >{projectSubtitle(repoCount, sessionTypes, status.runActionPhase)}</span
        >
      {/if}
    </div>
  </div>
</div>

<style>
  .row-main {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    flex: 1;
    min-width: 0;
  }

  .row-main :global(svg) {
    flex-shrink: 0;
    width: 16px;
  }

  .row-main :global(svg.pr-status-merged) {
    stroke: var(--ui-success);
  }

  .row-main :global(svg.pr-status-conflict) {
    stroke: var(--ui-danger);
  }

  .row-main :global(svg.pr-status-checks-failing) {
    stroke: var(--ui-danger);
  }

  .row-main :global(svg.pr-status-draft) {
    stroke: var(--text-faint);
  }

  .row-main :global(svg.pr-status-clean) {
    stroke: var(--text-faint);
  }

  .row-main :global(svg.cloud-running) {
    stroke: var(--ui-accent);
  }

  .row-main :global(svg.cloud-starting) {
    stroke: var(--ui-info);
  }

  .row-main :global(svg.cloud-error) {
    stroke: var(--ui-danger);
  }

  .row-main :global(svg.cloud-inactive) {
    stroke: var(--text-muted);
  }

  .row-text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .project-name {
    font-size: var(--size-sm);
    font-weight: 600;
    color: inherit;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 14px;
    font-size: calc(var(--size-xs) - 1px);
    line-height: 14px;
    color: var(--project-row-meta-color, var(--text-faint));
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .repo-count {
    color: var(--project-row-meta-color, var(--text-faint));
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .badge-row {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    min-width: 0;
    max-width: 100%;
    min-height: 14px;
    overflow: hidden;
    white-space: nowrap;
  }

  .activity-separator {
    color: var(--text-faint);
    margin: 0 1px;
  }

  .activity-text {
    color: var(--text-faint);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
