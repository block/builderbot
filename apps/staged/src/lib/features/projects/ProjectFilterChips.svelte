<!--
  ProjectFilterChips.svelte - Shared project-filter chip bar.

  Unread/running status chips plus one chip per repo, reading and toggling
  projectFiltersStore directly so the landing page and the sidebar's filter
  row can never drift. `compact` shrinks the chips for the narrow sidebar.
-->
<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { badgeBg, badgeBgHover, badgeFg } from '../../shared/badgeColors';
  import { darkMode } from '../../stores/isDark.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { projectFiltersStore } from './projectFilters.svelte';

  interface Props {
    /** Smaller chips, tighter gaps for the narrow sidebar. */
    compact?: boolean;
  }

  let { compact = false }: Props = $props();
</script>

<div class="filter-bar" class:compact>
  <button
    class="filter-chip"
    class:active={projectFiltersStore.isFilterActive('unread')}
    onclick={(e: MouseEvent) => projectFiltersStore.toggleFilter('unread', e)}
    disabled={projectFiltersStore.unreadCount === 0 &&
      !projectFiltersStore.isFilterActive('unread')}
  >
    Unread
    <span class="filter-count">{projectFiltersStore.unreadCount}</span>
  </button>
  <button
    class="filter-chip"
    class:active={projectFiltersStore.isFilterActive('running')}
    onclick={(e: MouseEvent) => projectFiltersStore.toggleFilter('running', e)}
    disabled={projectFiltersStore.runningCount === 0 &&
      !projectFiltersStore.isFilterActive('running')}
  >
    Running
    <span class="filter-count">{projectFiltersStore.runningCount}</span>
  </button>
  {#each projectFiltersStore.repoFilters as rf}
    {@const badge = repoBadgeStore.lookup(rf.repo, rf.subpath || undefined)}
    {@const filter = { repo: rf.repo, subpath: rf.subpath }}
    {@const active = projectFiltersStore.isFilterActive(filter)}
    <button
      class="filter-chip repo-filter"
      class:active
      onclick={(e: MouseEvent) => projectFiltersStore.toggleFilter(filter, e)}
      style={badge
        ? `--repo-bg: ${badgeBg(badge.hue, darkMode.value)}; --repo-fg: ${badgeFg(badge.hue, darkMode.value)}; --repo-bg-hover: ${badgeBgHover(badge.hue, darkMode.value)}`
        : ''}
    >
      <RepoLabel githubRepo={rf.repo} subpath={rf.subpath || null} />
      <span class="filter-count">{rf.count}</span>
    </button>
  {/each}
  <!--
    The filters outlive this bar's chips, so clearing has to be reachable from
    the bar itself: repo chips only exist for repos computeRepoFilters still
    sees (delete the last project using one and its filter stays active with
    no chip to click), and both status chips go disabled when their counts hit
    0. The sidebar's ✕ covers desktop, but the sidebar is hidden on mobile.
  -->
  {#if projectFiltersStore.hasActiveFilters}
    <button
      class="filter-chip clear-chip"
      title="Clear filters"
      onclick={() => projectFiltersStore.clearFilters()}
    >
      <X size={compact ? 11 : 12} />
      Clear
    </button>
  {/if}
</div>

<style>
  .filter-bar {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 14px;
  }

  .filter-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border: 1px solid var(--border-muted);
    border-radius: 999px;
    background: var(--bg-elevated);
    color: var(--text-secondary);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    white-space: nowrap;
  }

  .filter-chip:hover:not(:disabled) {
    background: var(--bg-hover);
    border-color: var(--border-emphasis);
  }

  .filter-chip.active:hover:not(:disabled) {
    background: var(--ui-accent);
    border-color: var(--ui-accent);
  }

  .filter-chip:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .filter-chip.active {
    background: var(--ui-accent);
    border-color: var(--ui-accent);
    color: white;
  }

  .filter-chip.repo-filter {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    font-size: 11px;
    font-weight: 600;
    background: var(--repo-bg, var(--bg-elevated));
    color: var(--repo-fg, var(--text-secondary));
    border-color: transparent;
  }

  .filter-chip.repo-filter:hover:not(:disabled) {
    background: var(--repo-bg-hover, var(--bg-hover));
  }

  .filter-chip.repo-filter.active {
    box-shadow: 0 0 0 2px var(--repo-fg, var(--ui-accent));
    background: var(--repo-bg, var(--ui-accent));
    color: var(--repo-fg, white);
    border-color: transparent;
  }

  .filter-chip.clear-chip {
    gap: 4px;
    color: var(--text-muted);
  }

  .filter-chip.clear-chip:hover {
    color: var(--text-primary);
  }

  .filter-count {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: 999px;
    background: rgba(128, 128, 128, 0.15);
    font-size: 11px;
    font-weight: 600;
    line-height: 1;
  }

  .filter-chip.active .filter-count {
    background: rgba(255, 255, 255, 0.25);
  }

  .filter-chip.repo-filter .filter-count {
    background: rgba(128, 128, 128, 0.15);
  }

  .filter-chip.repo-filter.active .filter-count {
    background: rgba(128, 128, 128, 0.2);
  }

  .filter-chip.repo-filter :global(.repo-label-prefix) {
    color: inherit;
    opacity: 0.6;
  }

  .filter-chip.repo-filter :global(.repo-label-emphasis) {
    color: inherit;
  }

  .filter-bar.compact {
    gap: 4px;
    margin-bottom: 0;
  }

  .filter-bar.compact .filter-chip {
    gap: 5px;
    padding: 2px 8px;
    font-size: var(--size-xs);
  }

  .filter-bar.compact .filter-chip.repo-filter {
    font-size: 10px;
  }

  .filter-bar.compact .filter-count {
    min-width: 15px;
    height: 15px;
    padding: 0 4px;
    font-size: 10px;
  }

  /* The sidebar (compact) never renders on mobile, so only the landing-page
     bar needs the swipeable single-row treatment. */
  @media (max-width: 640px) {
    .filter-bar:not(.compact) {
      flex-wrap: nowrap;
      gap: 8px;
      overflow-x: auto;
      margin: 0 -16px 14px;
      padding: 0 16px 4px;
    }

    .filter-bar:not(.compact) .filter-chip {
      min-height: 36px;
    }
  }
</style>
