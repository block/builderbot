<!--
  BoardColumnMenu.svelte — a board column's ⋯ menu: collapse it to a narrow
  strip, rename or recolour its tag (edits the tag itself, everywhere it
  shows), or hide the column. "No ‹group›" has no tag, so it only collapses
  and hides.
-->
<script lang="ts">
  import Ellipsis from '@lucide/svelte/icons/ellipsis';
  import EyeOff from '@lucide/svelte/icons/eye-off';
  import FoldHorizontal from '@lucide/svelte/icons/fold-horizontal';
  import Palette from '@lucide/svelte/icons/palette';
  import Pencil from '@lucide/svelte/icons/pencil';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import type { TagColor } from '../../types';
  import { tagActions } from '../tags/tagActions.svelte';
  import { TAG_COLORS, TAG_COLOR_VARS } from '../tags/tagColors';
  import type { BoardColumn } from './boardColumns';

  interface Props {
    column: BoardColumn;
    /** False on the phone pager, where columns are always full width. */
    canCollapse: boolean;
    onCollapse: () => void;
    onHide: () => void;
    /** Rename was picked; called once the menu has closed, so the name field can take focus. */
    onRename: () => void;
  }

  let { column, canCollapse, onCollapse, onHide, onRename }: Props = $props();

  let renameRequested = false;

  function colorLabel(color: TagColor): string {
    return color.charAt(0).toUpperCase() + color.slice(1);
  }

  function setColor(value: string) {
    const tag = column.tag;
    if (!tag || value === tag.color) return;
    void tagActions.editTag(tag.id, { color: value as TagColor });
  }

  function handleCloseAutoFocus(event: Event) {
    if (!renameRequested) return;
    renameRequested = false;
    event.preventDefault();
    onRename();
  }
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger
    class="board-column-menu-trigger touch-hit"
    title="Column actions"
    aria-label={`Actions for ${column.name}`}
  >
    <Ellipsis size={14} />
  </DropdownMenu.Trigger>
  <DropdownMenu.Content align="end" class="min-w-[180px]" onCloseAutoFocus={handleCloseAutoFocus}>
    {#if canCollapse}
      <DropdownMenu.Item onSelect={onCollapse}>
        <FoldHorizontal size={14} />
        Collapse
      </DropdownMenu.Item>
    {/if}
    {#if column.tag}
      <DropdownMenu.Item onSelect={() => (renameRequested = true)}>
        <Pencil size={14} />
        Rename
      </DropdownMenu.Item>
      <DropdownMenu.Sub>
        <DropdownMenu.SubTrigger>
          <Palette size={14} />
          Colour
        </DropdownMenu.SubTrigger>
        <DropdownMenu.SubContent class="min-w-[140px]">
          <DropdownMenu.RadioGroup value={column.tag.color} onValueChange={setColor}>
            {#each TAG_COLORS as color (color)}
              <DropdownMenu.RadioItem value={color}>
                <span class="color-dot" style:--dot-color={TAG_COLOR_VARS[color]}></span>
                {colorLabel(color)}
              </DropdownMenu.RadioItem>
            {/each}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.SubContent>
      </DropdownMenu.Sub>
    {/if}
    {#if canCollapse || column.tag}
      <DropdownMenu.Separator />
    {/if}
    <DropdownMenu.Item onSelect={onHide}>
      <EyeOff size={14} />
      Hide column
    </DropdownMenu.Item>
  </DropdownMenu.Content>
</DropdownMenu.Root>

<style>
  :global(.board-column-menu-trigger) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }

  :global(.board-column-menu-trigger:hover),
  :global(.board-column-menu-trigger[data-state='open']) {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .color-dot {
    width: 10px;
    height: 10px;
    flex-shrink: 0;
    border-radius: 50%;
    background: var(--dot-color);
  }
</style>
