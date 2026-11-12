<!--
  ProjectMatchReason.svelte — the one line a search result shows when the
  project matched through something other than its name or repos: the best
  contributing item's icon and label with the query's terms highlighted, and
  how many other items contributed besides (see searchProjects and
  matchEntry in projectSearch.ts).

  Shared by the landing-grid cards and the sidebar rows. Single line,
  ellipsised; the parent sets the font size and colour. A long label starts
  a few words before the first highlight of a term the item itself matched,
  so the ellipsis can't hide it (see matchReasonLabel).
-->
<script lang="ts">
  import FileSearch from '@lucide/svelte/icons/file-search';
  import FileText from '@lucide/svelte/icons/file-text';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
  import TagIcon from '@lucide/svelte/icons/tag';
  import { clampText, labelTerms, matchLabelSegments } from './matchReasonLabel';
  import type { MatchReason, MatchReasonKind } from './projectSearch';

  interface Props {
    reason: MatchReason;
    /** The query's terms (queryTerms), highlighted in the label. */
    terms: readonly string[];
    iconSize?: number;
  }

  let { reason, terms, iconSize = 12 }: Props = $props();

  const ICONS = {
    branch: GitBranch,
    pr: GitPullRequest,
    tag: TagIcon,
    note: FileText,
    projectNote: FileText,
    review: FileSearch,
  } satisfies Record<MatchReasonKind, unknown>;

  const KIND_NAMES: Record<MatchReasonKind, string> = {
    branch: 'Branch',
    pr: 'Pull request',
    tag: 'Tag',
    note: 'Note',
    projectNote: 'Project note',
    review: 'Review',
  };

  let Icon = $derived(ICONS[reason.kind]);
  let segments = $derived(
    matchLabelSegments(reason.label, labelTerms(terms), labelTerms(reason.itemTerms))
  );
  let tooltip = $derived(`${KIND_NAMES[reason.kind]}: ${clampText(reason.label)}`);
</script>

<span class="match-reason" title={tooltip}>
  <Icon size={iconSize} />
  <span class="sr-only">{KIND_NAMES[reason.kind]}:</span>
  <span class="match-label"
    >{#each segments as segment}{#if segment.match}<mark>{segment.text}</mark
        >{:else}{segment.text}{/if}{/each}</span
  >
  {#if reason.more > 0}
    <span class="match-more">+{reason.more} more</span>
  {/if}
</span>

<style>
  .match-reason {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
    white-space: nowrap;
  }

  .match-reason :global(svg) {
    flex-shrink: 0;
  }

  .match-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .match-label mark {
    background-color: var(--search-match-bg);
    border-radius: 2px;
    color: inherit;
  }

  .match-more {
    flex-shrink: 0;
    color: var(--text-faint);
  }
</style>
