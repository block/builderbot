<!--
  CommandPaletteRow.svelte — one project in the ⌘K palette's result list: its
  status icon (ProjectStatusIcon, as in the sidebar and the grid), its name
  with the query's matches highlighted, and a second line with its repo path
  and, when the query found it through a branch, PR, note or review, that
  reason (ProjectMatchReason, as the sidebar shows it). A project found by a
  repo's badge short name shows that name before the path.

  A listbox option: it takes no focus (the palette's input keeps it and points
  at the highlighted row through aria-activedescendant), so the row is a plain
  element with the palette's click handler; the palette's list follows the
  pointer over its rows. Trailing badges say what the palette can't otherwise
  show about a project it lists regardless of the window's filters: that it
  is the one on screen, that the window's filters hide it, or that it is
  being removed (and so can't be opened).
  While the palette waits to open the row (the diff view is saving a comment
  first) a spinner takes their place.
-->
<script lang="ts">
  import { Badge } from '$lib/components/ui/badge';
  import Spinner from '../../shared/Spinner.svelte';
  import type { HighlightSegment } from '../../shared/textSearch';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import ProjectMatchReason from '../projects/ProjectMatchReason.svelte';
  import ProjectStatusIcon from '../projects/ProjectStatusIcon.svelte';
  import RemoteProjectIcon from '../projects/RemoteProjectIcon.svelte';
  import type { PaletteRow } from './paletteSearch';

  interface Props {
    id: string;
    row: PaletteRow;
    /** The query's terms, highlighted in the match reason. */
    terms: readonly string[];
    selected: boolean;
    /** The project the window is showing. */
    current?: boolean;
    /** The window's own list (the sidebar or the grid) isn't showing this
     *  project: its chip filters or its search field hide it. */
    filteredOut?: boolean;
    /** A remove is in flight: listed, but not openable. */
    removing?: boolean;
    /** The palette is waiting to open this project: the diff view is saving
     *  a comment first. */
    busy?: boolean;
    onopen: () => void;
  }

  let {
    id,
    row,
    terms,
    selected,
    current = false,
    filteredOut = false,
    removing = false,
    busy = false,
    onopen,
  }: Props = $props();

  let project = $derived(row.project);
  let branches = $derived(projectsDataStore.branchesByProject.get(project.id) ?? []);
</script>

{#snippet highlighted(segments: readonly HighlightSegment[])}
  {#each segments as segment}{#if segment.match}<mark class="label-match">{segment.text}</mark
      >{:else}{segment.text}{/if}{/each}
{/snippet}

<!-- Not focusable on purpose (see above): the input owns the keyboard, and
     the click is the pointer's equivalent of its Enter. -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
  {id}
  class="palette-row"
  class:removing
  role="option"
  tabindex="-1"
  aria-selected={selected}
  aria-disabled={removing || undefined}
  aria-busy={busy || undefined}
  data-project-id={project.id}
  onclick={onopen}
>
  <span class="row-icon">
    <ProjectStatusIcon
      {project}
      {branches}
      hydrated={projectsDataStore.isProjectHydrated(project.id)}
      size={14}
    />
  </span>
  <span class="row-text">
    <span class="row-title">
      <span class="row-name">{@render highlighted(row.name)}</span>
      {#if project.location === 'remote'}
        <RemoteProjectIcon size={12} />
      {/if}
    </span>
    {#if row.repo || row.reason}
      <span class="row-meta">
        {#if row.repo}
          {#if row.repoAlias}
            <span class="row-alias">{@render highlighted(row.repoAlias)}</span>
          {/if}
          <span class="row-repo">{@render highlighted(row.repo)}</span>
          {#if row.moreRepos > 0}
            <span class="row-more">+{row.moreRepos}</span>
          {/if}
        {/if}
        {#if row.reason}
          {#if row.repo}<span class="row-separator" aria-hidden="true">&middot;</span>{/if}
          <span class="row-reason">
            <ProjectMatchReason reason={row.reason} {terms} iconSize={11} />
          </span>
        {/if}
      </span>
    {/if}
  </span>
  {#if busy}
    <span class="row-busy">
      <Spinner size={12} />
      Saving comment…
    </span>
  {:else if removing || current || filteredOut}
    <span class="row-badges">
      {#if removing}
        <Badge variant="outline" class="palette-badge">Removing…</Badge>
      {:else}
        {#if current}
          <Badge variant="outline" class="palette-badge">Current</Badge>
        {/if}
        {#if filteredOut}
          <Badge
            variant="outline"
            class="palette-badge"
            title="Hidden by this window's filters or search"
          >
            Filtered out
          </Badge>
        {/if}
      {/if}
    </span>
  {/if}
</div>

<style>
  .palette-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-radius: 8px;
    color: var(--text-primary);
    cursor: pointer;
    /* Keeps a row revealed by the arrow keys clear of the list's padding. */
    scroll-margin: 6px;
  }

  .palette-row[aria-selected='true'] {
    background-color: var(--ui-selection);
  }

  .palette-row.removing {
    cursor: default;
    opacity: 0.55;
  }

  .row-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 16px;
  }

  .row-text {
    display: flex;
    flex-direction: column;
    gap: 3px;
    flex: 1;
    min-width: 0;
  }

  .row-title {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .row-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--size-sm);
    font-weight: 600;
    line-height: 1.35;
  }

  .row-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font-size: var(--size-xs);
    line-height: 1.35;
    color: var(--text-muted);
    white-space: nowrap;
  }

  .row-repo {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    /* The path's end (the subpath) is what tells two repos apart, and it is
       what the name line doesn't already say; let the reason give way first. */
    flex-shrink: 1;
  }

  /* The badge short name the query found the repo by, set off from the path
     the way the app's repo badges are: a small outlined label. */
  .row-alias {
    flex-shrink: 0;
    padding: 0 5px;
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-secondary);
  }

  .row-more,
  .row-separator {
    flex-shrink: 0;
    color: var(--text-faint);
  }

  .row-reason {
    display: flex;
    flex: 1 2 0;
    min-width: 0;
  }

  /* Matches use the global `label-match` rule, shared with ProjectMatchReason,
     which draws the reason beside these. The pill's border sits right under
     the text; pull the line up off it. */
  .row-alias mark {
    text-underline-offset: 0.5px;
  }

  .row-badges {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .row-badges :global(.palette-badge) {
    height: 18px;
    padding: 0 6px;
    border-color: var(--border-muted);
    color: var(--text-muted);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
  }

  /* Fades in after a beat, like the sidebar's busy spinner, so a save that
     returns at once doesn't flash it. */
  .row-busy {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: var(--size-xs);
    white-space: nowrap;
    animation: row-busy-in 300ms 150ms both;
  }

  .row-busy :global(.spinner) {
    color: var(--ui-accent);
  }

  @keyframes row-busy-in {
    from {
      opacity: 0;
    }
  }

  /* The selection fill sits close to the standard border in dark themes, so
     the selected row's badges take the stronger border to keep their outline. */
  .palette-row[aria-selected='true'] .row-badges :global(.palette-badge) {
    border-color: var(--border-emphasis);
  }
</style>
