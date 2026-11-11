<!--
  TagGroupSection.svelte — one tag group in Settings › Tags: its name, which
  renames in place, a ⋯ menu, and its tags in order (the board's column
  order).

  The menu renames, makes the group the project icon (its tag's icon then
  leads every sidebar row and grid card), pins the group's board to the
  projects sidebar (a row under the Projects heading opens it), moves the
  group up or down, and deletes it after asking what happens to its tags.

  Like a tag's name (TagRow), the group name keeps what's being typed while
  it has focus, whatever catalog echoes arrive meanwhile.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import ArrowDown from '@lucide/svelte/icons/arrow-down';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import Ellipsis from '@lucide/svelte/icons/ellipsis';
  import Pencil from '@lucide/svelte/icons/pencil';
  import SquareKanban from '@lucide/svelte/icons/square-kanban';
  import Star from '@lucide/svelte/icons/star';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import type { Tag, TagGroup } from '../../../types';
  import { MAX_TAG_NAME_CHARS } from '../../tags/tagState';
  import TagList from './TagList.svelte';
  import { groupMenuTriggerId, groupNameInputId, type TagsEditor } from './tagsEditor.svelte';

  interface Props {
    editor: TagsEditor;
    group: TagGroup;
    tags: Tag[];
    canMoveUp: boolean;
    canMoveDown: boolean;
  }

  let { editor, group, tags, canMoveUp, canMoveDown }: Props = $props();

  // "Rename" in the menu focuses the name once the menu has closed, rather
  // than letting the menu hand focus back to its trigger.
  let renameRequested = false;

  let nameDraft = $state(untrack(() => group.name));
  let nameFocused = $state(false);
  $effect(() => {
    const name = group.name;
    if (!untrack(() => nameFocused)) nameDraft = name;
  });

  async function commitName() {
    nameFocused = false;
    const name = nameDraft.trim();
    if (!name || name === group.name) {
      nameDraft = group.name;
      return;
    }
    if (!(await editor.renameGroup(group, name))) nameDraft = group.name;
  }

  function handleNameKeydown(event: KeyboardEvent & { currentTarget: HTMLInputElement }) {
    if (event.key === 'Enter') {
      event.currentTarget.blur();
    } else if (event.key === 'Escape') {
      nameDraft = group.name;
      event.currentTarget.blur();
    }
  }

  function handleMenuCloseAutoFocus(event: Event) {
    if (!renameRequested) return;
    renameRequested = false;
    event.preventDefault();
    const input = document.getElementById(groupNameInputId(group.id));
    if (input instanceof HTMLInputElement) {
      input.focus();
      input.select();
    }
  }
</script>

<section class="tag-group" aria-labelledby={groupNameInputId(group.id)}>
  <header class="group-head">
    <input
      id={groupNameInputId(group.id)}
      class="group-name"
      type="text"
      aria-label="Group name"
      maxlength={MAX_TAG_NAME_CHARS}
      bind:value={nameDraft}
      onfocus={() => (nameFocused = true)}
      onblur={() => void commitName()}
      onkeydown={handleNameKeydown}
    />
    {#if group.isIconGroup}
      <span class="icon-badge" title="Each project's tag in this group is its icon">
        <Star size={11} />
        Project icon
      </span>
    {/if}
    {#if group.showInSidebar}
      <span class="icon-badge" title="This group's board is pinned to the projects sidebar">
        <SquareKanban size={11} />
        In sidebar
      </span>
    {/if}
    <DropdownMenu.Root>
      <DropdownMenu.Trigger
        id={groupMenuTriggerId(group.id)}
        class="tag-group-menu-trigger touch-hit"
        title="Group actions"
        aria-label={`Actions for ${group.name}`}
      >
        <Ellipsis size={15} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Content
        align="end"
        class="min-w-[200px]"
        onCloseAutoFocus={handleMenuCloseAutoFocus}
      >
        <DropdownMenu.Item onSelect={() => (renameRequested = true)}>
          <Pencil size={14} />
          Rename
        </DropdownMenu.Item>
        <DropdownMenu.CheckboxItem
          checked={group.isIconGroup}
          onCheckedChange={(checked) => editor.setIconGroup(group, checked)}
        >
          Show as project icon
        </DropdownMenu.CheckboxItem>
        <DropdownMenu.CheckboxItem
          checked={group.showInSidebar}
          onCheckedChange={(checked) => editor.setSidebarBoard(group, checked)}
        >
          Show board in sidebar
        </DropdownMenu.CheckboxItem>
        <DropdownMenu.Separator />
        <DropdownMenu.Item disabled={!canMoveUp} onSelect={() => void editor.moveGroup(group, -1)}>
          <ArrowUp size={14} />
          Move up
        </DropdownMenu.Item>
        <DropdownMenu.Item disabled={!canMoveDown} onSelect={() => void editor.moveGroup(group, 1)}>
          <ArrowDown size={14} />
          Move down
        </DropdownMenu.Item>
        <DropdownMenu.Separator />
        <DropdownMenu.Item variant="destructive" onSelect={() => editor.requestDeleteGroup(group)}>
          <Trash2 size={14} />
          Delete group…
        </DropdownMenu.Item>
      </DropdownMenu.Content>
    </DropdownMenu.Root>
  </header>

  <TagList
    {editor}
    groupId={group.id}
    {tags}
    label={`${group.name} tags`}
    emptyText="No tags yet. Add one, or drag a tag here."
  />
</section>

<style>
  .tag-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .group-head {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  /* Reads as the section heading; editable in place. */
  .group-name {
    min-width: 0;
    max-width: 320px;
    flex: 0 1 auto;
    field-sizing: content;
    margin-left: -6px;
    padding: 2px 6px;
    border: 1px solid transparent;
    border-radius: 5px;
    background: transparent;
    color: var(--text-primary);
    font: inherit;
    font-size: var(--size-sm);
    font-weight: 600;
    text-overflow: ellipsis;
  }

  .group-name:hover {
    border-color: var(--border-subtle);
  }

  .group-name:focus {
    outline: none;
    border-color: var(--border-emphasis);
    background: var(--bg-elevated);
  }

  .icon-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    padding: 1px 7px;
    border: 1px solid var(--border-subtle);
    border-radius: 999px;
    color: var(--text-muted);
    font-size: calc(var(--size-xs) - 1px);
    white-space: nowrap;
  }

  :global(.tag-group-menu-trigger) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 24px;
    height: 24px;
    margin-left: auto;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }

  :global(.tag-group-menu-trigger:hover),
  :global(.tag-group-menu-trigger[data-state='open']) {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  @media (pointer: coarse) {
    :global(.tag-group-menu-trigger) {
      width: 32px;
      height: 32px;
    }
  }
</style>
