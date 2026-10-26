<!--
  SidebarFilterMenu.svelte - "Filter ▾" dropdown for the sidebar's projects header.

  A Status group (Unread, Running) and a Repo group with project counts, all
  reading and toggling projectFiltersStore so the landing page's chip bar and
  this menu can never drift. Menu checkboxes toggle additively (the chips'
  shift-click semantics): a menu is a place to stack filters, and exclusive
  selection would make "Unread + my repo" impossible to express here. The
  trigger shows how many filters are active, and a Clear filters item appears
  while any are — repo filters can outlive their menu entry (delete the last
  project using a repo and its filter stays active with nothing to uncheck),
  so clearing has to be reachable from the menu itself.
-->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ListFilter from '@lucide/svelte/icons/list-filter';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import RepoBadge from '../../shared/RepoBadge.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { projectFiltersStore, type FilterKind } from './projectFilters.svelte';
  import { onWorkspaceParked } from '../layout/navigation.svelte';

  let open = $state(false);

  // Parked under settings or the diff view, the menu would stay open above them.
  onWorkspaceParked(() => {
    open = false;
  });

  let activeCount = $derived(projectFiltersStore.activeFilters.size);

  // Checkbox semantics: toggle within the current set.
  function toggle(filter: FilterKind) {
    projectFiltersStore.toggleFilter(filter, { additive: true });
  }

  // The repo entries and their counts derive from reposByProject, which
  // otherwise only fills via the store's idle drip — the landing page forces
  // hydration but the sidebar never did, so kick it when the menu opens or it
  // could render missing or undercounted when the app opens straight into a
  // project. Re-runs when a reload lands while open.
  $effect(() => {
    if (!open || !projectsDataStore.loaded) return;
    void projectsDataStore.ensureProjectsHydrated();
  });
</script>

<DropdownMenu.Root bind:open>
  <DropdownMenu.Trigger
    class="sidebar-filter-trigger {activeCount > 0 ? 'filtering' : ''}"
    title="Filter projects"
  >
    <ListFilter size={12} />
    {activeCount > 0 ? `${activeCount} filter${activeCount === 1 ? '' : 's'}` : 'Filter'}
    <ChevronDown size={11} />
  </DropdownMenu.Trigger>
  <DropdownMenu.Content align="end" sideOffset={4} class="min-w-[210px]">
    <DropdownMenu.Label>Status</DropdownMenu.Label>
    <DropdownMenu.CheckboxItem
      checked={projectFiltersStore.isFilterActive('unread')}
      closeOnSelect={false}
      disabled={projectFiltersStore.unreadCount === 0 &&
        !projectFiltersStore.isFilterActive('unread')}
      onCheckedChange={() => toggle('unread')}
    >
      Unread <span class="menu-count">{projectFiltersStore.unreadCount}</span>
    </DropdownMenu.CheckboxItem>
    <DropdownMenu.CheckboxItem
      checked={projectFiltersStore.isFilterActive('running')}
      closeOnSelect={false}
      disabled={projectFiltersStore.runningCount === 0 &&
        !projectFiltersStore.isFilterActive('running')}
      onCheckedChange={() => toggle('running')}
    >
      Running <span class="menu-count">{projectFiltersStore.runningCount}</span>
    </DropdownMenu.CheckboxItem>
    {#if projectFiltersStore.repoFilters.length > 0}
      <DropdownMenu.Separator />
      <DropdownMenu.Label>Repo</DropdownMenu.Label>
      {#each projectFiltersStore.repoFilters as rf (rf.repo + '\t' + rf.subpath)}
        {@const badge = repoBadgeStore.lookup(rf.repo, rf.subpath || undefined)}
        {@const filter = { repo: rf.repo, subpath: rf.subpath }}
        <DropdownMenu.CheckboxItem
          checked={projectFiltersStore.isFilterActive(filter)}
          closeOnSelect={false}
          onCheckedChange={() => toggle(filter)}
        >
          <!-- Badges only exist for a repo's githubRepo while filter keys use
               headRepo ?? githubRepo, so a fork-backed repo falls back to
               its path instead of dropping the entry. -->
          {#if badge}
            <RepoBadge shortName={badge.shortName} hue={badge.hue} small />
          {:else}
            <span class="repo-fallback">
              <RepoLabel githubRepo={rf.repo} subpath={rf.subpath || null} />
            </span>
          {/if}
          <span class="menu-count">{rf.count}</span>
        </DropdownMenu.CheckboxItem>
      {/each}
    {/if}
    {#if projectFiltersStore.hasActiveFilters}
      <DropdownMenu.Separator />
      <DropdownMenu.Item onSelect={() => projectFiltersStore.clearFilters()}>
        Clear filters
      </DropdownMenu.Item>
    {/if}
  </DropdownMenu.Content>
</DropdownMenu.Root>

<style>
  /* DropdownMenu.Trigger renders in a child component, so its class escapes
     this component's scoping; the descendants below are authored here and
     stay scoped. */
  :global(.sidebar-filter-trigger) {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-weight: 500;
    padding: 3px 6px;
    cursor: pointer;
    white-space: nowrap;
    transition:
      background-color 0.15s ease,
      color 0.15s ease;
  }

  :global(.sidebar-filter-trigger:hover),
  :global(.sidebar-filter-trigger[data-state='open']) {
    background: var(--projects-sidebar-hover-bg, var(--bg-hover));
    color: var(--text-primary);
  }

  :global(.sidebar-filter-trigger.filtering) {
    color: var(--ui-accent);
  }

  .menu-count {
    margin-left: auto;
    /* Clear the checkbox indicator parked in the item's right padding. */
    padding-right: 4px;
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }

  .repo-fallback {
    display: inline-flex;
    align-items: center;
    max-width: 150px;
    overflow: hidden;
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    font-size: 11px;
  }
</style>
