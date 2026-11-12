<!--
  BoardMoveToMenu.svelte — "Move to ▸" in a board card's context menu: the
  board group's radio submenu under the board's own column names ("No
  ‹group›" first). It's how a card changes column without dragging (touch,
  phone, keyboard), so it leads the menu. The write is optimistic, and
  `onMoved` lets the board put focus back on the card in its new column.
-->
<script lang="ts">
  import Columns3 from '@lucide/svelte/icons/columns-3';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import type { Project, TagGroup } from '../../types';
  import { tagsStore } from '../../stores/tags.svelte';
  import TagIcon from '../tags/TagIcon.svelte';
  import { tagActions } from '../tags/tagActions.svelte';
  import { NO_TAG_MENU_VALUE } from '../tags/tagState';
  import { noTagColumnName } from './boardColumns';

  interface Props {
    project: Project;
    /** The board's group. */
    group: TagGroup;
    /** After a pick that changed the project's column. */
    onMoved?: (projectId: string) => void;
  }

  let { project, group, onMoved }: Props = $props();

  let current = $derived(tagsStore.groupTagFor(project.id, group.id));
  let tags = $derived(tagsStore.tagsByGroup.get(group.id) ?? []);

  function moveTo(value: string) {
    const next = value === NO_TAG_MENU_VALUE ? null : value;
    if ((current?.id ?? null) === next) return;
    // Optimistic: the card is in its new column as soon as this returns.
    void tagActions.setProjectGroupTag(project, group.id, next);
    onMoved?.(project.id);
  }
</script>

<ContextMenu.Sub>
  <ContextMenu.SubTrigger data-tag-group-trigger="move-to">
    <Columns3 size={14} />
    <span class="min-w-0 flex-1 truncate">Move to</span>
  </ContextMenu.SubTrigger>
  <ContextMenu.SubContent class="min-w-[200px]">
    <ContextMenu.RadioGroup value={current?.id ?? NO_TAG_MENU_VALUE} onValueChange={moveTo}>
      <ContextMenu.RadioItem value={NO_TAG_MENU_VALUE}>
        <span class="truncate">{noTagColumnName(group)}</span>
      </ContextMenu.RadioItem>
      {#each tags as tag (tag.id)}
        <ContextMenu.RadioItem value={tag.id}>
          <TagIcon {tag} size={14} />
          <span class="truncate">{tag.name}</span>
        </ContextMenu.RadioItem>
      {/each}
    </ContextMenu.RadioGroup>
  </ContextMenu.SubContent>
</ContextMenu.Sub>
