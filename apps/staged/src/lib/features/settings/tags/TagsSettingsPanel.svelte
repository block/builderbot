<!--
  TagsSettingsPanel.svelte — Settings › Tags: every tag group with its tags,
  then the ungrouped tags.

  A group is single-choice (a project has at most one of its tags), and its
  tag order is the board's column order. One group can be the project icon:
  its tag's icon leads each sidebar row and grid card, and a project with no
  tag there shows its computed PR status. Tags drag to reorder or to move
  between groups; each row's ⋯ menu does the same without a pointer. Groups
  drag to reorder too, from the handle on their header, and their ⋯ menu
  moves them up or down. The group order is the one every list of groups in
  the app follows.

  Each group section takes a group dropped on it. The list also takes one
  dropped between sections or below them, where the drop line is drawn: it
  lands where the last section it crossed put that line. The line goes once
  the drag leaves the list.

  State and actions live in a TagsEditor (see tagsEditor.svelte.ts) that the
  sections and rows share.
-->
<script lang="ts">
  import Info from '@lucide/svelte/icons/info';
  import Tags from '@lucide/svelte/icons/tags';
  import { dragStillInside } from '../../../shared/dragStillInside';
  import { tagsStore } from '../../../stores/tags.svelte';
  import DeleteTagDialog from './DeleteTagDialog.svelte';
  import DeleteTagGroupDialog from './DeleteTagGroupDialog.svelte';
  import MoveTagConfirmDialog from './MoveTagConfirmDialog.svelte';
  import NewTagGroupMenu from './NewTagGroupMenu.svelte';
  import TagGroupSection from './TagGroupSection.svelte';
  import UngroupedTagsSection from './UngroupedTagsSection.svelte';
  import { TagsEditor } from './tagsEditor.svelte';

  const editor = new TagsEditor();

  let body = $state<HTMLElement | null>(null);

  let catalog = $derived(editor.catalog);

  let deleteTag = $derived(editor.deleteTagTarget);
  let deleteGroup = $derived(editor.deleteGroupTarget);
  let move = $derived(editor.moveConfirmTarget);
  let moveTagName = $derived(move ? (catalog.tagsById.get(move.tagId)?.name ?? '') : '');

  // A group dragged into the gap between two sections, or past the last one.
  // The sections take their own events; these are the ones no section saw,
  // and they keep the target the last section named. Nothing is accepted
  // unless a drop would move the group, so the cursor says when it won't.
  // Text and files are not a group, even with a group drag left over.
  function handleGroupDragOver(event: DragEvent) {
    if (!editor.isGroupDrag(event) || !editor.groupDropWouldMove) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
  }

  // A dragleave bubbles from everything the drag crosses in the list. Only
  // one that takes it out of the list forgets the target: over the sidebar
  // or outside the window a release does nothing, so no line should say
  // otherwise, and a drag that comes back has to cross a section again.
  function handleGroupDragLeave(event: DragEvent) {
    if (editor.groupDropTarget === null || !body || dragStillInside(body, event)) return;
    editor.groupDragLeave();
  }

  function handleGroupDrop(event: DragEvent) {
    if (!editor.isGroupDrag(event) || !editor.groupDropWouldMove) return;
    event.preventDefault();
    editor.groupDrop();
  }
</script>

<div class="tags-settings-panel">
  <div class="panel-intro">
    <div class="intro-copy">
      <h2>
        <Tags size={16} />
        Tags
      </h2>
      <p>
        Group and label projects. A project has at most one tag from each group. Groups appear in
        this order across the app.
      </p>
    </div>
    <NewTagGroupMenu
      disabled={editor.creatingGroup}
      onCreate={(template) => void editor.createGroup(template)}
    />
  </div>

  <div
    bind:this={body}
    class="panel-body"
    role="presentation"
    ondragover={handleGroupDragOver}
    ondragleave={handleGroupDragLeave}
    ondrop={handleGroupDrop}
  >
    {#if !tagsStore.loaded}
      <p class="panel-note">Loading tags…</p>
    {:else}
      {#each catalog.groups as group, i (group.id)}
        <TagGroupSection
          {editor}
          {group}
          tags={catalog.tagsByGroup.get(group.id) ?? []}
          canMoveUp={i > 0}
          canMoveDown={i < catalog.groups.length - 1}
        />
      {:else}
        <p class="panel-note">
          <Info size={12} />
          No groups yet. Start one from New group; Status recreates the project statuses.
        </p>
      {/each}

      <UngroupedTagsSection {editor} tags={catalog.tagsByGroup.get(null) ?? []} />

      {#if catalog.groups.length > 0 && !catalog.iconGroup}
        <p class="panel-note">
          <Info size={12} />
          No group is the project icon, so every project shows its PR status. Choose one from a group's
          menu.
        </p>
      {/if}
    {/if}
  </div>
</div>

<DeleteTagDialog
  bind:open={editor.deleteTagOpen}
  tag={deleteTag}
  usage={deleteTag ? editor.usage(deleteTag.id) : 0}
  inIconGroup={!!deleteTag &&
    deleteTag.groupId !== null &&
    deleteTag.groupId === catalog.iconGroup?.id}
  onConfirm={() => void editor.confirmDeleteTag()}
/>

<DeleteTagGroupDialog
  bind:open={editor.deleteGroupOpen}
  group={deleteGroup}
  tagCount={deleteGroup ? (catalog.tagsByGroup.get(deleteGroup.id)?.length ?? 0) : 0}
  usage={deleteGroup ? editor.groupUsage(deleteGroup.id) : 0}
  onConfirm={(mode) => void editor.confirmDeleteGroup(mode)}
/>

<MoveTagConfirmDialog
  bind:open={editor.moveConfirmOpen}
  tagName={moveTagName}
  groupName={move ? editor.scopeName(move.groupId) : ''}
  conflicts={move?.conflicts ?? 0}
  onConfirm={() => void editor.confirmMove()}
/>

<style>
  .tags-settings-panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg-primary);
  }

  .panel-intro {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 14px 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .intro-copy {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }

  .panel-intro h2 {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-base);
    font-weight: 600;
  }

  .panel-intro p {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .panel-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 16px;
    /* The phone layout's bottom inset (SettingsPage's `.panel-pad`). */
    padding-bottom: calc(16px + var(--settings-panel-inset-bottom, 0px));
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  /* Rows stay readable on a wide window rather than stretching the name. */
  .panel-body > :global(*) {
    max-width: 720px;
  }

  .panel-note {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.4;
    display: flex;
    align-items: baseline;
    gap: 4px;
  }
</style>
