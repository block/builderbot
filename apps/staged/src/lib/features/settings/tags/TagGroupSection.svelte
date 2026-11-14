<!--
  TagGroupSection.svelte — one tag group in Settings › Tags: a drag handle,
  its name, which renames in place, a ⋯ menu, and its tags in order (the
  board's column order).

  The menu renames, makes the group the project icon (its tag's icon then
  leads every sidebar row and grid card), pins the group's board to the
  projects sidebar (a row under the Projects heading opens it), moves the
  group up or down, and deletes it after asking what happens to its tags.

  The handle drags the group among the others, and the whole section is the
  drop zone: a group dropped on its upper half lands above it, on its lower
  half below. The drag is made of pointer events (shared/pointerDrag), not
  an HTML5 drag, which the desktop build's native file-drop handler would
  take. The panel resolves every point the pointer reaches, from the
  sections' rects, so the section only says which group is being dragged and
  draws the line when the group would land beside it. A tag's drag is still
  an HTML5 one, and is the rows' and the list's to take.

  Like a tag's name (TagRow), the group name keeps what's being typed while
  it has focus, whatever catalog echoes arrive meanwhile.
-->
<script module lang="ts">
  import type { PointerDragPoint, PointerDragScroll } from '../../../shared/pointerDrag';

  /** What every group section shares for dragging groups, from TagsSettingsPanel. */
  export interface TagGroupDragHandlers {
    /** The list of sections, which a drag scrolls near its top or bottom edge. */
    readonly scroll: PointerDragScroll;
    /** A drag of this group began. `drop` or `cancel` always follows. */
    start(groupId: string): void;
    /** The pointer is at `point`: whether a drop there would move the group. */
    move(point: PointerDragPoint): boolean;
    /** The group was released at `point`. */
    drop(point: PointerDragPoint): void;
    /** The drag ended without a release. */
    cancel(): void;
  }
</script>

<script lang="ts">
  import { untrack } from 'svelte';
  import ArrowDown from '@lucide/svelte/icons/arrow-down';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import Ellipsis from '@lucide/svelte/icons/ellipsis';
  import GripVertical from '@lucide/svelte/icons/grip-vertical';
  import Pencil from '@lucide/svelte/icons/pencil';
  import SquareKanban from '@lucide/svelte/icons/square-kanban';
  import Star from '@lucide/svelte/icons/star';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { pointerDrag, type PointerDragOptions } from '../../../shared/pointerDrag';
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
    groupDrag: TagGroupDragHandlers;
  }

  let { editor, group, tags, canMoveUp, canMoveDown, groupDrag }: Props = $props();

  let dropPlace = $derived(editor.groupDropPlaceFor(group.id));

  // What the handle hands the drag action. The panel resolves every point,
  // so the section only says which group is being dragged.
  let dragOptions = $derived<PointerDragOptions>({
    scroll: groupDrag.scroll,
    start: () => groupDrag.start(group.id),
    move: (point) => groupDrag.move(point),
    drop: (point) => groupDrag.drop(point),
    cancel: () => groupDrag.cancel(),
  });

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

<section
  class="tag-group"
  class:dragging={editor.dragGroupId === group.id}
  class:drop-before={dropPlace === 'before'}
  class:drop-after={dropPlace === 'after'}
  data-tag-group={group.id}
  aria-labelledby={groupNameInputId(group.id)}
>
  <header class="group-head">
    <!-- Dragging is the pointer's way to move a group. The ⋯ menu's Move up
         and Move down are the keyboard's, so the handle takes no role or tab
         stop for it. -->
    <span
      class="drag-handle"
      title="Drag to reorder groups"
      aria-hidden="true"
      use:pointerDrag={dragOptions}
    >
      <GripVertical size={14} />
    </span>
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
        <DropdownMenu.Item disabled={!canMoveUp} onSelect={() => editor.moveGroup(group, -1)}>
          <ArrowUp size={14} />
          Move up
        </DropdownMenu.Item>
        <DropdownMenu.Item disabled={!canMoveDown} onSelect={() => editor.moveGroup(group, 1)}>
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
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .tag-group.dragging {
    opacity: 0.4;
  }

  /* The drop indicator: a line midway across the gap to the next section
     (the panel body's 24px), where a tag row draws its own in the row gap. */
  .tag-group.drop-before::before,
  .tag-group.drop-after::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    height: 2px;
    border-radius: 1px;
    background: var(--ui-accent);
    pointer-events: none;
  }

  .tag-group.drop-before::before {
    top: -13px;
  }

  .tag-group.drop-after::after {
    bottom: -13px;
  }

  .group-head {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  /* The same handle a tag row has, in the same column. The press that drags
     it must not start a selection on the way, and the cursor during the
     drag is the root's (see app.css). */
  .drag-handle {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 16px;
    height: 24px;
    color: var(--text-faint);
    cursor: grab;
    -webkit-user-select: none;
    user-select: none;
  }

  .drag-handle:hover {
    color: var(--text-muted);
  }

  .drag-handle:active {
    cursor: grabbing;
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

  /* As on a tag row: no drag on a phone or a touch screen, where the ⋯
     menu's Move up and Move down reorder instead. */
  @media (max-width: 640px), (pointer: coarse) {
    .drag-handle {
      display: none;
    }
  }

  @media (pointer: coarse) {
    :global(.tag-group-menu-trigger) {
      width: 32px;
      height: 32px;
    }
  }
</style>
