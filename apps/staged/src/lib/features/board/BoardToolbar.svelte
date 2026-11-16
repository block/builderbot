<!--
  BoardToolbar.svelte — the board's own controls: "Board by [group ▾]", and
  "Hidden columns (n)" to bring back columns hidden from their menus. In the
  home page's search row on desktop; at the top of the phone pager. A pinned
  board (BoardView) fixes its group by route, so it leaves the picker out.
  The triggers wear `board-toolbar-trigger` (app.css), as the sort menu
  beside them does.
-->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import CircleDashed from '@lucide/svelte/icons/circle-dashed';
  import Eye from '@lucide/svelte/icons/eye';
  import EyeOff from '@lucide/svelte/icons/eye-off';
  import Settings2 from '@lucide/svelte/icons/settings-2';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import type { TagGroup } from '../../types';
  import { tagsStore } from '../../stores/tags.svelte';
  import { openSettings } from '../layout/navigation.svelte';
  import TagIcon from '../tags/TagIcon.svelte';
  import { hiddenBoardColumns } from './boardColumns';
  import { columnPrefsFor } from './boardPrefs';
  import { boardViewPrefs, setBoardColumnFlag, setBoardGroup } from './boardViewPrefs.svelte';

  interface Props {
    group: TagGroup;
    /** Offer "Board by [group ▾]"; off where the route fixes the group. */
    groupPicker?: boolean;
  }

  let { group, groupPicker = true }: Props = $props();

  let hidden = $derived(
    hiddenBoardColumns(
      group,
      tagsStore.tagsByGroup.get(group.id) ?? [],
      columnPrefsFor(boardViewPrefs.columns, group.id, tagsStore.catalog).hidden
    )
  );

  function show(keys: string[]) {
    for (const key of keys) {
      setBoardColumnFlag(group.id, key, 'hidden', false, tagsStore.catalog);
    }
  }
</script>

<div class="board-toolbar">
  {#if groupPicker}
    <DropdownMenu.Root>
      <DropdownMenu.Trigger
        class="board-toolbar-trigger touch-hit"
        title="Choose the group to board by"
      >
        <span class="trigger-label">Board by</span>
        <span class="trigger-value">{group.name}</span>
        <ChevronDown size={13} class="board-toolbar-chevron" />
      </DropdownMenu.Trigger>
      <DropdownMenu.Content align="start" class="min-w-[180px]">
        <DropdownMenu.Label>Board by</DropdownMenu.Label>
        <DropdownMenu.RadioGroup value={group.id} onValueChange={(value) => setBoardGroup(value)}>
          {#each tagsStore.groups as option (option.id)}
            <DropdownMenu.RadioItem value={option.id}>
              <span class="truncate">{option.name}</span>
            </DropdownMenu.RadioItem>
          {/each}
        </DropdownMenu.RadioGroup>
        <DropdownMenu.Separator />
        <DropdownMenu.Item onSelect={() => openSettings('tags')}>
          <Settings2 size={14} />
          Manage tags…
        </DropdownMenu.Item>
      </DropdownMenu.Content>
    </DropdownMenu.Root>
  {/if}

  {#if hidden.length > 0}
    <DropdownMenu.Root>
      <DropdownMenu.Trigger class="board-toolbar-trigger touch-hit" title="Show hidden columns">
        <EyeOff size={13} class="board-toolbar-chevron" />
        <span class="trigger-value">Hidden columns ({hidden.length})</span>
      </DropdownMenu.Trigger>
      <DropdownMenu.Content align="end" class="min-w-[180px]">
        <DropdownMenu.Label>Show column</DropdownMenu.Label>
        {#each hidden as column (column.key)}
          <DropdownMenu.Item onSelect={() => show([column.key])}>
            {#if column.tag}
              <TagIcon tag={column.tag} size={14} />
            {:else}
              <CircleDashed size={14} class="text-muted-foreground" />
            {/if}
            <span class="truncate">{column.name}</span>
          </DropdownMenu.Item>
        {/each}
        {#if hidden.length > 1}
          <DropdownMenu.Separator />
          <DropdownMenu.Item onSelect={() => show(hidden.map((column) => column.key))}>
            <Eye size={14} />
            Show all
          </DropdownMenu.Item>
        {/if}
      </DropdownMenu.Content>
    </DropdownMenu.Root>
  {/if}
</div>

<style>
  .board-toolbar {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
</style>
