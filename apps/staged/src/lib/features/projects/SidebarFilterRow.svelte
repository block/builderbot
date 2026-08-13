<!--
  SidebarFilterRow.svelte - Compact project-filter row for the projects sidebar.

  Collapsed: one line summarizing the shared filter state — muted "Filter
  projects" when nothing is active; otherwise the active status filters as
  text, the active repo filters as badges, and a matched/total count, plus a
  ✕ that clears everything without expanding. Expanded: the same
  ProjectFilterChips bar the landing page renders, so the two surfaces can't
  drift. Only the open/closed state is local — the selection itself lives in
  projectFiltersStore.
-->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import ListFilter from '@lucide/svelte/icons/list-filter';
  import X from '@lucide/svelte/icons/x';
  import RepoBadge from '../../shared/RepoBadge.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { filterKey, projectFiltersStore } from './projectFilters.svelte';
  import ProjectFilterChips from './ProjectFilterChips.svelte';

  let expanded = $state(false);

  let statusLabels = $derived.by(() => {
    const labels: string[] = [];
    if (projectFiltersStore.isFilterActive('unread')) labels.push('Unread');
    if (projectFiltersStore.isFilterActive('running')) labels.push('Running');
    return labels;
  });

  // A badge lookup can miss — badges are only ensured for a repo's
  // githubRepo, while filter keys use headRepo ?? githubRepo, so a
  // fork-backed repo has no badge under its head name. Fall back to the repo
  // path instead of dropping the entry: a filter that names nothing leaves
  // this row showing a funnel and a bare count, which is the one thing it
  // exists to explain.
  let activeRepos = $derived(
    projectFiltersStore.activeRepoFilters
      .map((filter) => {
        const badge = repoBadgeStore.lookup(filter.repo, filter.subpath || undefined);
        return { filter, badge, sortKey: badge?.shortName ?? filter.repo };
      })
      .sort((a, b) => a.sortKey.localeCompare(b.sortKey))
  );

  // The repo chips and their counts derive from reposByProject, which
  // otherwise only fills via the store's idle drip — the landing page forces
  // hydration but the sidebar never did, so kick it when the chips become
  // visible or they could render missing or undercounted when the app opens
  // straight into a project. Re-runs when a reload lands while expanded.
  $effect(() => {
    if (!expanded || !projectsDataStore.loaded) return;
    void projectsDataStore.ensureProjectsHydrated();
  });
</script>

<div class="filter-row" class:filtering={projectFiltersStore.hasActiveFilters}>
  <button
    class="summary-button"
    aria-expanded={expanded}
    title={expanded ? 'Hide filter options' : 'Filter projects'}
    onclick={() => (expanded = !expanded)}
  >
    <span class="filter-icon"><ListFilter size={13} /></span>
    {#if projectFiltersStore.hasActiveFilters}
      <span class="summary">
        {#if statusLabels.length > 0}
          <span class="status-labels">{statusLabels.join(', ')}</span>
        {/if}
        {#if activeRepos.length > 0}
          <span class="badge-row">
            {#each activeRepos as { filter, badge } (filterKey(filter))}
              {#if badge}
                <RepoBadge shortName={badge.shortName} hue={badge.hue} small />
              {:else}
                <span class="repo-fallback">
                  <RepoLabel githubRepo={filter.repo} subpath={filter.subpath || null} />
                </span>
              {/if}
            {/each}
          </span>
        {/if}
      </span>
      <span class="match-count">
        {projectFiltersStore.filteredProjects.length}/{projectsDataStore.projects.length}
      </span>
    {:else}
      <span class="summary placeholder">Filter projects</span>
    {/if}
    <span class="chevron">
      {#if expanded}
        <ChevronDown size={12} />
      {:else}
        <ChevronRight size={12} />
      {/if}
    </span>
  </button>
  {#if projectFiltersStore.hasActiveFilters}
    <button
      class="clear-button"
      aria-label="Clear filters"
      title="Clear filters"
      onclick={() => projectFiltersStore.clearFilters()}
    >
      <X size={12} />
    </button>
  {/if}
</div>

{#if expanded}
  <div class="chips-container">
    <ProjectFilterChips compact />
  </div>
{/if}

<style>
  .filter-row {
    display: flex;
    align-items: center;
    gap: 2px;
    width: calc(100% + (2 * var(--project-row-bleed)));
    margin: 0 calc(-1 * var(--project-row-bleed));
  }

  .summary-button {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
    min-width: 0;
    border: none;
    border-radius: 0;
    background: transparent;
    color: var(--text-muted);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
    cursor: pointer;
    padding: 5px 10px;
    text-align: left;
    transition:
      background-color 0.15s ease,
      color 0.15s ease;
  }

  .summary-button:hover {
    background-color: var(--projects-sidebar-hover-bg);
    color: var(--text-primary);
  }

  .filter-icon {
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
  }

  .filter-row.filtering .filter-icon {
    color: var(--ui-accent);
  }

  .summary {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
  }

  .summary.placeholder {
    color: inherit;
  }

  .status-labels {
    flex-shrink: 0;
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .badge-row {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    min-width: 0;
    min-height: 14px;
    overflow: hidden;
  }

  .repo-fallback {
    display: inline-flex;
    align-items: center;
    max-width: 96px;
    overflow: hidden;
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    font-size: 9.5px;
  }

  .match-count {
    flex-shrink: 0;
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }

  .chevron {
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
    color: var(--text-faint);
  }

  .clear-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 22px;
    height: 22px;
    margin-right: 4px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    padding: 0;
    transition:
      background-color 0.15s ease,
      color 0.15s ease;
  }

  .clear-button:hover {
    background-color: var(--projects-sidebar-hover-bg);
    color: var(--text-primary);
  }

  .chips-container {
    padding: 2px 2px 6px;
  }
</style>
