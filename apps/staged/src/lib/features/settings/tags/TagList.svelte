<!--
  TagList.svelte — one scope's tags in Settings › Tags (a group's, or the
  ungrouped ones), with Add tag below.

  The whole list is a drop zone: a tag dropped on a row lands beside it, and
  one dropped anywhere else in the list lands at the end, which is how a tag
  reaches an empty group.
-->
<script lang="ts">
  import Plus from '@lucide/svelte/icons/plus';
  import { Button } from '$lib/components/ui/button';
  import type { Tag } from '../../../types';
  import TagRow from './TagRow.svelte';
  import { UNGROUPED_LABEL, type TagsEditor } from './tagsEditor.svelte';

  interface Props {
    editor: TagsEditor;
    /** The scope: a group id, or null for ungrouped tags. */
    groupId: string | null;
    tags: Tag[];
    /** Shown in place of the rows when the scope has no tags. */
    emptyText: string;
    /** Names the list for assistive tech. */
    label: string;
  }

  let { editor, groupId, tags, emptyText, label }: Props = $props();

  let moveTargets = $derived([
    ...editor.catalog.groups
      .filter((group) => group.id !== groupId)
      .map((group) => ({ id: group.id as string | null, name: group.name })),
    ...(groupId === null ? [] : [{ id: null, name: UNGROUPED_LABEL }]),
  ]);
  let dropAtEnd = $derived(editor.dropsAtEndOf(groupId));

  function handleDragOver(event: DragEvent) {
    if (editor.dragTagId === null) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
    editor.dragOver({ groupId, anchorTagId: null });
  }

  function handleDrop(event: DragEvent) {
    if (editor.dragTagId === null) return;
    event.preventDefault();
    editor.drop();
  }
</script>

<div class="tag-list" role="group" ondragover={handleDragOver} ondrop={handleDrop}>
  {#if tags.length > 0}
    <ul class="rows" class:drop-end={dropAtEnd} aria-label={label}>
      {#each tags as tag, i (tag.id)}
        <TagRow {editor} {tag} {moveTargets} canMoveUp={i > 0} canMoveDown={i < tags.length - 1} />
      {/each}
    </ul>
  {:else}
    <p class="empty" class:drop-end={dropAtEnd}>{emptyText}</p>
  {/if}
  <div class="list-actions">
    <Button
      variant="ghost"
      size="sm"
      disabled={editor.isAddingTag(groupId)}
      onclick={() => void editor.addTag(groupId)}
    >
      <Plus size={14} />
      Add tag
    </Button>
  </div>
</div>

<style>
  .tag-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .rows {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  /* Dropping past the last row: the same line a row draws, under the list. */
  .rows.drop-end::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    bottom: -3px;
    height: 2px;
    border-radius: 1px;
    background: var(--ui-accent);
    pointer-events: none;
  }

  .empty {
    margin: 0;
    padding: 10px 12px;
    border: 1px dashed var(--border-muted);
    border-radius: 6px;
    font-size: var(--size-xs);
    color: var(--text-muted);
    transition:
      border-color 0.1s ease,
      background-color 0.1s ease;
  }

  .empty.drop-end {
    border-color: var(--ui-accent);
    background: color-mix(in srgb, var(--ui-accent) 8%, transparent);
  }

  .list-actions {
    display: flex;
    align-items: center;
    margin-left: -8px;
  }
</style>
