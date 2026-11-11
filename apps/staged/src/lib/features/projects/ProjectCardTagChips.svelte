<!--
  ProjectCardTagChips.svelte — a project grid card's tags: up to three chips
  for the tags its status icon doesn't already show (every group but the icon
  group, in group order, then the ungrouped tags), then "+N" for the rest.
  Renders nothing for a project without such tags. Sidebar rows have no room
  for chips and show only the icon. A board card also leaves out the board's
  group (`excludeGroupId`), which its column already shows.
-->
<script lang="ts">
  import { tagsStore } from '../../stores/tags.svelte';
  import TagChip from '../tags/TagChip.svelte';
  import { limitChips } from '../tags/tagState';

  /** Chips a card shows before "+N". */
  const MAX_CARD_CHIPS = 3;

  interface Props {
    projectId: string;
    /** A group to leave out besides the icon group. */
    excludeGroupId?: string | null;
  }

  let { projectId, excludeGroupId = null }: Props = $props();

  let allChips = $derived(
    tagsStore.chipTagsFor(projectId, {
      excludeGroupIds: [tagsStore.iconGroup?.id, excludeGroupId],
    })
  );
  let chips = $derived(limitChips(allChips, MAX_CARD_CHIPS));
  let hiddenNames = $derived(
    allChips
      .slice(MAX_CARD_CHIPS)
      .map((tag) => tag.name)
      .join(', ')
  );
</script>

{#if chips.shown.length > 0}
  <div class="card-tag-chips">
    {#each chips.shown as tag (tag.id)}
      <TagChip {tag} size="sm" />
    {/each}
    {#if chips.overflow > 0}
      <span class="card-tag-more" title={hiddenNames}>+{chips.overflow}</span>
    {/if}
  </div>
{/if}

<style>
  .card-tag-chips {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    min-width: 0;
  }

  .card-tag-more {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 600;
  }
</style>
