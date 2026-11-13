<!--
  ProjectTagChips.svelte — a project's tags as chips, for the line that
  already names its repos: a card's labels (ProjectCardLabels) and a row's
  meta line (ProjectRowContent). Up to `max` chips for the tags the status
  icon doesn't already show (every group but the icon group, in group order,
  then the ungrouped tags), then "+N" for the rest.

  Renders bare chips with no wrapper, so they are items of the parent's
  cluster and wrap, shrink and space like the repo badges beside them; a
  project without such tags renders nothing. A board card also leaves out the
  board's group (`excludeGroupId`), whose tag the card's column stands for.
-->
<script lang="ts">
  import { tagsStore } from '../../stores/tags.svelte';
  import TagChip from '../tags/TagChip.svelte';
  import { limitChips } from '../tags/tagState';

  interface Props {
    projectId: string;
    /** A group to leave out besides the icon group. */
    excludeGroupId?: string | null;
    /** Chips shown before "+N". */
    max?: number;
    /** `sm` for cards, `xs` for a row's meta line. */
    size?: 'xs' | 'sm';
  }

  let { projectId, excludeGroupId = null, max = 3, size = 'sm' }: Props = $props();

  let allChips = $derived(
    tagsStore.chipTagsFor(projectId, {
      excludeGroupIds: [tagsStore.iconGroup?.id, excludeGroupId],
    })
  );
  let chips = $derived(limitChips(allChips, max));
  let hiddenNames = $derived(
    allChips
      .slice(max)
      .map((tag) => tag.name)
      .join(', ')
  );
</script>

{#each chips.shown as tag (tag.id)}
  <TagChip {tag} {size} />
{/each}
{#if chips.overflow > 0}
  <span class="tag-chips-more" class:xs={size === 'xs'} title={hiddenNames}>+{chips.overflow}</span>
{/if}

<style>
  .tag-chips-more {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 600;
  }

  .tag-chips-more.xs {
    font-size: 9.5px;
    line-height: 1.3;
  }
</style>
