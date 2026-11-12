<!--
  SidebarFilterMenu.svelte - "Filter ▾" dropdown for the sidebar's projects header.

  An Activity group (Unread, Running; not "Status", which the default tag
  group is called), a Repo group, and the tags under a heading per tag group
  then "Tags" for the ungrouped ones (as in the tag picker), all
  with project counts, reading and toggling projectFiltersStore so the landing
  page's chip bar and this menu can never drift. Menu checkboxes toggle
  additively (the chips' shift-click semantics): a menu is a place to stack
  filters, and exclusive selection would make "Unread + my repo" impossible to
  express here. The trigger shows how many filters are active (a deleted tag's
  filter doesn't count), and a Clear filters item appears while any are — repo
  filters can outlive their menu entry (delete the last project using a repo
  and its filter stays active with nothing to uncheck), so clearing has to be
  reachable from the menu itself. It sits on the edge
  facing the trigger, the shortest reach from opening the menu, and keeps
  keyboard focus if the menu flips sides while it has it.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ListFilter from '@lucide/svelte/icons/list-filter';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import RepoBadge from '../../shared/RepoBadge.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import TagIcon from '../tags/TagIcon.svelte';
  import { projectFiltersStore, type FilterKind } from './projectFilters.svelte';
  import { onViewHidden } from '../layout/navigation.svelte';

  let open = $state(false);

  // Off screen (home shown over the project layer, or parked under settings /
  // the diff view), the menu would stay open on top.
  onViewHidden(() => {
    open = false;
  });

  let activeCount = $derived(projectFiltersStore.effectiveActiveFilters.size);

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

  // bits-ui opens the menu below the trigger and flips it above when the
  // window has no room underneath (a long pinned-repos list can push this
  // header to the bottom of the sidebar), reporting the side it settled on as
  // the content's data-side. Clear filters follows that side so it always
  // faces the trigger. Positioning settles before bits-ui's keyboard focus
  // lands (focus waits a frame), so a keyboard open still starts on the first
  // item of the final order. Sheet menus have no data-side and keep it on top.
  //
  // Moving it renders the other branch, a new node (not a CSS reorder:
  // bits-ui roves through items in DOM order, so arrow keys would run against
  // the visual order). If the flip happens while the old node has focus
  // (a resize or layout shift with the menu open), removing it would drop
  // focus out of the menu and with it arrow-key and Escape handling, so the
  // flip notes that and the new node takes focus as it mounts.
  let content = $state<HTMLElement | null>(null);
  let placedSide = $state<string | null>(null);
  let refocusClear = false;

  $effect(() => {
    const el = content;
    if (!el) {
      placedSide = null;
      return;
    }
    const read = () => {
      const side = el.getAttribute('data-side');
      const moving = (side === 'top') !== clearAtBottom;
      const focused = el.ownerDocument.activeElement;
      refocusClear = moving && !!focused?.matches('[data-clear-filters]') && el.contains(focused);
      placedSide = side;
    };
    // Untracked so reading clearAtBottom doesn't re-run this on every flip.
    untrack(read);
    const observer = new MutationObserver(read);
    observer.observe(el, { attributes: true, attributeFilter: ['data-side'] });
    return () => observer.disconnect();
  });

  let clearAtBottom = $derived(placedSide === 'top');

  // Runs once the new Clear filters node is in the document, in the same
  // update that removed the old one, so focus never lands anywhere else.
  function takeFocusFromFlip(node: HTMLElement) {
    if (!refocusClear) return;
    refocusClear = false;
    node.focus({ preventScroll: true });
  }
</script>

{#snippet clearItem()}
  <DropdownMenu.Item
    data-clear-filters
    {@attach takeFocusFromFlip}
    onSelect={() => projectFiltersStore.clearFilters()}
  >
    Clear filters
  </DropdownMenu.Item>
{/snippet}

<DropdownMenu.Root bind:open>
  <DropdownMenu.Trigger
    class="sidebar-filter-trigger {activeCount > 0 ? 'filtering' : ''}"
    title="Filter projects"
  >
    <ListFilter size={12} />
    {activeCount > 0 ? `${activeCount} filter${activeCount === 1 ? '' : 's'}` : 'Filter'}
    <ChevronDown size={11} />
  </DropdownMenu.Trigger>
  <DropdownMenu.Content bind:ref={content} align="end" sideOffset={4} class="min-w-[210px]">
    {#if projectFiltersStore.hasActiveFilters && !clearAtBottom}
      {@render clearItem()}
      <DropdownMenu.Separator />
    {/if}
    <DropdownMenu.Label>Activity</DropdownMenu.Label>
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
    {#each projectFiltersStore.tagFilterSections as section (section.key)}
      <DropdownMenu.Separator />
      <DropdownMenu.Label>{section.label}</DropdownMenu.Label>
      {#each section.filters as tf (tf.tag.id)}
        {@const filter = { tagId: tf.tag.id }}
        <DropdownMenu.CheckboxItem
          checked={projectFiltersStore.isFilterActive(filter)}
          closeOnSelect={false}
          onCheckedChange={() => toggle(filter)}
        >
          <TagIcon tag={tf.tag} size={12} />
          <span class="tag-name">{tf.tag.name}</span>
          <span class="menu-count">{tf.count}</span>
        </DropdownMenu.CheckboxItem>
      {/each}
    {/each}
    {#if projectFiltersStore.hasActiveFilters && clearAtBottom}
      <DropdownMenu.Separator />
      {@render clearItem()}
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

  .tag-name {
    min-width: 0;
    max-width: 150px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
