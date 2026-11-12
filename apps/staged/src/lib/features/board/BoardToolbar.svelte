<!--
  BoardToolbar.svelte — the board's own controls: "Board by [group ▾]", and
  "Hidden columns (n)" to bring back columns hidden from their menus. In the
  home page's search row on desktop; at the top of the phone pager. A pinned
  board (BoardView) fixes its group by route, so it leaves the picker out.
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

  :global(.board-toolbar-trigger) {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
    max-width: 240px;
    height: 32px;
    padding: 0 8px 0 10px;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease;
  }

  :global(.board-toolbar-trigger:hover),
  :global(.board-toolbar-trigger[data-state='open']) {
    background: var(--bg-hover);
    border-color: var(--border-emphasis);
  }

  :global(.board-toolbar-trigger .trigger-label) {
    color: var(--text-muted);
  }

  :global(.board-toolbar-trigger .trigger-value) {
    min-width: 0;
    overflow: hidden;
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  :global(.board-toolbar-trigger .board-toolbar-chevron) {
    flex-shrink: 0;
    color: var(--text-muted);
  }
</style>
