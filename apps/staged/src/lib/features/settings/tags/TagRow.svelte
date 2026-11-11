<!--
  TagRow.svelte — one tag in Settings › Tags: drag handle, icon, name,
  colour, how many projects use it, a ⋯ menu and delete.

  Projects carry a tag by id, so an edit here shows on every project at once.
  Each edit is a backend write, so the name saves on blur or Enter rather
  than per keystroke, and Escape puts it back. While the name field has
  focus it keeps what's being typed: a `tags-changed` echo (another window
  recolouring some tag) re-renders the row from the fresh catalog, and must
  not put the saved name back over the draft. Edits send only the field
  they change (tagActions.editTag fills in the rest from the store once the
  writes before it settle), so a rename followed at once by a colour or icon
  pick keeps both. The handle drags the row to reorder it or move it to
  another group; the ⋯ menu does the same from the keyboard or a touch
  screen.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import ArrowDown from '@lucide/svelte/icons/arrow-down';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import Ellipsis from '@lucide/svelte/icons/ellipsis';
  import FolderInput from '@lucide/svelte/icons/folder-input';
  import GripVertical from '@lucide/svelte/icons/grip-vertical';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import type { Tag } from '../../../types';
  import IconPicker from '../../actions/IconPicker.svelte';
  import TagIcon from '../../tags/TagIcon.svelte';
  import type { TagValue } from '../../tags/tagActions.svelte';
  import { tagActions } from '../../tags/tagActions.svelte';
  import { MAX_TAG_NAME_CHARS } from '../../tags/tagState';
  import TagColorSwatches from './TagColorSwatches.svelte';
  import { tagMenuTriggerId, tagNameInputId, type TagsEditor } from './tagsEditor.svelte';

  /** The icon picker's empty-search shortlist: statuses and priorities. */
  const TAG_ICONS = [
    'lightbulb',
    'circle-dot',
    'paintbrush',
    'eye',
    'circle-check',
    'circle-pause',
    'octagon-minus',
    'clock',
    'flag',
    'bug',
    'rocket',
    'hourglass',
    'shield-alert',
    'chevrons-up',
    'chevron-up',
    'minus',
    'chevron-down',
  ];

  interface Props {
    editor: TagsEditor;
    tag: Tag;
    /** The scopes "Move to group" offers: every one but the tag's own. */
    moveTargets: { id: string | null; name: string }[];
    canMoveUp: boolean;
    canMoveDown: boolean;
  }

  let { editor, tag, moveTargets, canMoveUp, canMoveDown }: Props = $props();

  let row = $state<HTMLLIElement | null>(null);
  let usage = $derived(editor.usage(tag.id));
  let dropPlace = $derived(editor.dropPlaceFor(tag.id));

  // The name field's draft. It follows the catalog except while the field
  // has focus, when it is the user's.
  let nameDraft = $state(untrack(() => tag.name));
  let nameFocused = $state(false);
  $effect(() => {
    const name = tag.name;
    if (!untrack(() => nameFocused)) nameDraft = name;
  });

  function save(patch: Partial<TagValue>): Promise<boolean> {
    return tagActions.editTag(tag.id, patch);
  }

  async function commitName() {
    nameFocused = false;
    const name = nameDraft.trim();
    // A name can't be blank; an unchanged one needs no write.
    if (!name || name === tag.name) {
      nameDraft = tag.name;
      return;
    }
    if (!(await save({ name }))) nameDraft = tag.name;
  }

  function handleNameKeydown(event: KeyboardEvent & { currentTarget: HTMLInputElement }) {
    if (event.key === 'Enter') {
      event.currentTarget.blur();
    } else if (event.key === 'Escape') {
      nameDraft = tag.name;
      event.currentTarget.blur();
    }
  }

  function handleDragStart(event: DragEvent) {
    if (event.dataTransfer) {
      event.dataTransfer.effectAllowed = 'move';
      // Some engines only start a drag that carries data.
      event.dataTransfer.setData('text/plain', tag.name);
      if (row) event.dataTransfer.setDragImage(row, 16, row.offsetHeight / 2);
    }
    editor.dragStart(tag.id);
  }

  function handleDragOver(event: DragEvent) {
    // Only drags that started on a tag row; files and text pass through.
    if (editor.dragTagId === null || !row) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
    const rect = row.getBoundingClientRect();
    editor.dragOver({
      groupId: tag.groupId,
      anchorTagId: tag.id,
      place: event.clientY < rect.top + rect.height / 2 ? 'before' : 'after',
    });
  }

  function handleDrop(event: DragEvent) {
    if (editor.dragTagId === null) return;
    event.preventDefault();
    event.stopPropagation();
    editor.drop();
  }
</script>

<li
  bind:this={row}
  class="tag-row"
  class:dragging={editor.dragTagId === tag.id}
  class:drop-before={dropPlace === 'before'}
  class:drop-after={dropPlace === 'after'}
  data-tag-row={tag.id}
  ondragover={handleDragOver}
  ondrop={handleDrop}
>
  <span
    class="drag-handle"
    draggable="true"
    title="Drag to reorder, or onto another group"
    aria-hidden="true"
    ondragstart={handleDragStart}
    ondragend={() => editor.dragEnd()}
  >
    <GripVertical size={14} />
  </span>
  <IconPicker
    icon={tag.icon}
    curated={TAG_ICONS}
    clearLabel="No icon"
    onSelect={(icon) => {
      if (icon !== tag.icon) void save({ icon });
    }}
  >
    {#snippet triggerIcon()}
      <TagIcon {tag} size={14} />
    {/snippet}
    {#snippet clearIcon()}
      <TagIcon tag={{ icon: null, color: tag.color }} size={14} />
    {/snippet}
  </IconPicker>
  <Input
    id={tagNameInputId(tag.id)}
    type="text"
    placeholder="Tag name"
    aria-label="Tag name"
    class="tag-name-input min-w-0"
    maxlength={MAX_TAG_NAME_CHARS}
    bind:value={nameDraft}
    onfocus={() => (nameFocused = true)}
    onblur={() => void commitName()}
    onkeydown={handleNameKeydown}
  />
  <TagColorSwatches color={tag.color} onSelect={(color) => void save({ color })} />
  <span class="usage" class:unused={usage === 0}>
    {usage === 0 ? 'Unused' : `Used by ${usage}`}
  </span>
  <DropdownMenu.Root>
    <DropdownMenu.Trigger
      id={tagMenuTriggerId(tag.id)}
      class="tag-row-menu-trigger touch-hit"
      title="More"
      aria-label={`More actions for ${tag.name}`}
    >
      <Ellipsis size={14} />
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="end" class="min-w-[180px]">
      <DropdownMenu.Item disabled={!canMoveUp} onSelect={() => editor.moveStep(tag.id, -1)}>
        <ArrowUp size={14} />
        Move up
      </DropdownMenu.Item>
      <DropdownMenu.Item disabled={!canMoveDown} onSelect={() => editor.moveStep(tag.id, 1)}>
        <ArrowDown size={14} />
        Move down
      </DropdownMenu.Item>
      {#if moveTargets.length > 0}
        <DropdownMenu.Sub>
          <DropdownMenu.SubTrigger>
            <FolderInput size={14} />
            Move to group
          </DropdownMenu.SubTrigger>
          <DropdownMenu.SubContent class="min-w-[160px]">
            {#each moveTargets as target (target.id ?? '')}
              <DropdownMenu.Item onSelect={() => editor.moveToScope(tag.id, target.id)}>
                <span class="truncate">{target.name}</span>
              </DropdownMenu.Item>
            {/each}
          </DropdownMenu.SubContent>
        </DropdownMenu.Sub>
      {/if}
    </DropdownMenu.Content>
  </DropdownMenu.Root>
  <Button
    variant="ghost"
    size="icon-sm"
    class="hover:text-destructive"
    title="Delete tag"
    aria-label={`Delete ${tag.name}`}
    onclick={() => editor.requestDeleteTag(tag)}
  >
    <Trash2 size={13} />
  </Button>
</li>

<style>
  .tag-row {
    position: relative;
    display: grid;
    grid-template-columns: auto auto minmax(80px, 1fr) auto 64px auto auto;
    align-items: center;
    gap: 8px;
    padding: 3px 0;
  }

  .tag-row.dragging {
    opacity: 0.4;
  }

  /* The drop indicator: a line in the gap above or below the row. */
  .tag-row.drop-before::before,
  .tag-row.drop-after::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    height: 2px;
    border-radius: 1px;
    background: var(--ui-accent);
    pointer-events: none;
  }

  .tag-row.drop-before::before {
    top: -2px;
  }

  .tag-row.drop-after::after {
    bottom: -2px;
  }

  .drag-handle {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 28px;
    color: var(--text-faint);
    cursor: grab;
  }

  .drag-handle:hover {
    color: var(--text-muted);
  }

  .drag-handle:active {
    cursor: grabbing;
  }

  .usage {
    font-size: var(--size-xs);
    color: var(--text-muted);
    white-space: nowrap;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .usage.unused {
    color: var(--text-faint);
  }

  :global(.tag-row-menu-trigger) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }

  :global(.tag-row-menu-trigger:hover),
  :global(.tag-row-menu-trigger[data-state='open']) {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  /* A phone has no room for one line: the name takes its own row under the
     handle and icon, and drag gives way to the ⋯ menu. */
  @media (max-width: 640px), (pointer: coarse) {
    .tag-row {
      grid-template-columns: auto minmax(0, 1fr) auto auto;
      grid-template-areas:
        'icon name menu delete'
        'swatches swatches usage usage';
      row-gap: 6px;
      padding: 6px 0;
    }

    .drag-handle {
      display: none;
    }

    .tag-row > :global(.icon-picker-trigger) {
      grid-area: icon;
    }

    .tag-row > :global(.tag-name-input) {
      grid-area: name;
    }

    .tag-row > :global(.swatches) {
      grid-area: swatches;
    }

    .usage {
      grid-area: usage;
    }

    .tag-row > :global(.tag-row-menu-trigger) {
      grid-area: menu;
      width: 32px;
      height: 32px;
    }

    .tag-row > :global([data-slot='button']) {
      grid-area: delete;
      min-width: 32px;
      min-height: 32px;
    }
  }
</style>
