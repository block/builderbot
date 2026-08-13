<!--
  SidebarFilterRow.svelte - Compact project-filter row for the projects sidebar.

  Collapsed: one line summarizing the shared filter state — muted "Filter
  projects" when nothing is active; otherwise the active status filters as
  text, the active repo filters as badges, and a matched/total count, plus a
  ✕ that clears everything without expanding. The row is a Popover trigger:
  opening it floats the same ProjectFilterChips bar the landing page renders
  in an elevated dropdown under the row (so the two surfaces can't drift),
  dismissed by clicking anywhere outside. Only the open/closed state is
  local — the selection itself lives in projectFiltersStore.

  The row is sticky: once the sidebar scrolls past its natural position it
  pins to the top of the scroll area, so the filter summary (and the way out
  of a filtered-down list) stays reachable from anywhere in a long list.
-->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import ListFilter from '@lucide/svelte/icons/list-filter';
  import X from '@lucide/svelte/icons/x';
  import * as Popover from '$lib/components/ui/popover';
  import RepoBadge from '../../shared/RepoBadge.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { filterKey, projectFiltersStore } from './projectFilters.svelte';
  import ProjectFilterChips from './ProjectFilterChips.svelte';

  let open = $state(false);
  let stuck = $state(false);

  // The hairline under the row should only exist while it's pinned, and CSS
  // has no pinned-state selector for sticky elements. With the scroll
  // container's top edge inset by 1px, being clipped at the *top* is unique
  // to the pinned state — clipped at the bottom just means the row scrolled
  // below the fold — and an observer also catches pinning caused by content
  // above the row changing height, which fires no scroll event.
  function observeStuck(node: HTMLElement) {
    let scrollParent = node.parentElement;
    while (scrollParent && !/auto|scroll/.test(getComputedStyle(scrollParent).overflowY)) {
      scrollParent = scrollParent.parentElement;
    }
    if (!scrollParent) return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        stuck =
          entry.intersectionRatio < 1 &&
          entry.boundingClientRect.top <= (entry.rootBounds?.top ?? Number.NEGATIVE_INFINITY);
      },
      { root: scrollParent, threshold: 1, rootMargin: '-1px 0px 0px 0px' }
    );
    observer.observe(node);
    return {
      destroy() {
        observer.disconnect();
      },
    };
  }

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
  // straight into a project. Re-runs when a reload lands while open.
  $effect(() => {
    if (!open || !projectsDataStore.loaded) return;
    void projectsDataStore.ensureProjectsHydrated();
  });
</script>

<Popover.Root bind:open>
  <div
    class="filter-row"
    class:filtering={projectFiltersStore.hasActiveFilters}
    class:stuck
    use:observeStuck
  >
    <Popover.Trigger
      class="sidebar-filter-trigger"
      title={open ? 'Hide filter options' : 'Filter projects'}
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
        {#if open}
          <ChevronDown size={12} />
        {:else}
          <ChevronRight size={12} />
        {/if}
      </span>
    </Popover.Trigger>
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
  <Popover.Content
    align="start"
    sideOffset={4}
    class="sidebar-filter-panel w-[var(--bits-popover-anchor-width)] ring-0"
  >
    <ProjectFilterChips compact />
  </Popover.Content>
</Popover.Root>

<style>
  .filter-row {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 2px;
    width: calc(100% + (2 * var(--project-row-bleed)));
    margin: 0 calc(-1 * var(--project-row-bleed));
    /* Opaque so rows scrolling under the pinned row are masked, invisible at
       rest since it matches the sidebar. */
    background: var(--bg-app-bar);
  }

  .filter-row.stuck {
    box-shadow: 0 1px 0 color-mix(in srgb, var(--border-subtle) 50%, transparent);
  }

  /* Popover.Trigger renders in a child component, so its class escapes this
     component's scoping; the descendants below are authored here and stay
     scoped. */
  :global(.sidebar-filter-trigger) {
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

  :global(.sidebar-filter-trigger:hover) {
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

  /* Unlayered, so it wins over the Tailwind utility defaults (p-4, gap-4,
     bg-popover) baked into Popover.Content — same pattern as the settings
     panel's theme dropdown. */
  :global(.sidebar-filter-panel) {
    gap: 0;
    padding: 8px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    box-shadow: var(--shadow-elevated);
  }
</style>
