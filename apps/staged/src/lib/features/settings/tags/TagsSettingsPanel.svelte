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

  A group's drag is made of pointer events (shared/pointerDrag), because the
  desktop build's native file-drop handler takes every HTML5 drag. The panel
  resolves each point the pointer reaches from the sections' rects, by how
  far down the list it is alone: the handle is at the list's left edge, and
  a pointer that drifts off it is still dragging. Level with a section, the
  group lands above or below it. Between two sections, where the drop line
  is drawn, it lands between them. Below the last section it lands last, and
  above the first, first. Clearly away from the list (see `DRAG_REACH`) it
  lands nowhere and the line goes. Held near the top or bottom of the list,
  or just past it, the group scrolls it. A chip with the group's name
  follows the pointer.

  A drop leaves the list scrolled where it was, and then scrolls it only as
  far as it takes to show the group that moved. The list opts out of the
  engine's scroll anchoring for that (see `.panel-body`).

  State and actions live in a TagsEditor (see tagsEditor.svelte.ts) that the
  sections and rows share.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import Info from '@lucide/svelte/icons/info';
  import Tags from '@lucide/svelte/icons/tags';
  import type { PointerDragPoint } from '../../../shared/pointerDrag';
  import { tagsStore } from '../../../stores/tags.svelte';
  import type { GroupDropTarget } from '../../tags/tagReorder';
  import DeleteTagDialog from './DeleteTagDialog.svelte';
  import DeleteTagGroupDialog from './DeleteTagGroupDialog.svelte';
  import MoveTagConfirmDialog from './MoveTagConfirmDialog.svelte';
  import NewTagGroupMenu from './NewTagGroupMenu.svelte';
  import TagGroupSection, { type TagGroupDragHandlers } from './TagGroupSection.svelte';
  import UngroupedTagsSection from './UngroupedTagsSection.svelte';
  import { groupAtPoint, groupRevealOffset } from './tagSettingsDrag';
  import { TagsEditor } from './tagsEditor.svelte';

  const editor = new TagsEditor();

  let body = $state<HTMLElement | null>(null);

  let catalog = $derived(editor.catalog);

  let deleteTag = $derived(editor.deleteTagTarget);
  let deleteGroup = $derived(editor.deleteGroupTarget);
  let move = $derived(editor.moveConfirmTarget);
  let moveTagName = $derived(move ? (catalog.tagsById.get(move.tagId)?.name ?? '') : '');

  /**
   * Where a group held at `point` would land (see `groupAtPoint`), from the
   * list and the sections as they are laid out now. The drag scrolls the
   * list, so the rects are read afresh each time.
   */
  function groupDropAt(point: PointerDragPoint): GroupDropTarget | null {
    if (!body) return null;
    const sections = [...body.querySelectorAll<HTMLElement>('[data-tag-group]')].map((el) => ({
      groupId: el.dataset.tagGroup ?? '',
      rect: el.getBoundingClientRect(),
    }));
    return groupAtPoint(point, body.getBoundingClientRect(), sections, editor.groupDropTarget);
  }

  /** Tell the editor where a group held at `point` would land, or that it would land nowhere. */
  function holdGroupAt(point: PointerDragPoint) {
    const target = groupDropAt(point);
    if (target) editor.groupDragOver(target);
    else editor.groupDragLeave();
  }

  /**
   * Scroll the list, and only the list, as far as it takes to show a group
   * that a drop just moved (see `groupRevealOffset`). The drop can be made
   * with the list scrolled, and can move a group that had scrolled out of
   * view, or one too tall to be seen whole from where the pointer let go.
   */
  async function revealGroup(groupId: string) {
    // The section is where the drop put it once the new order has rendered.
    await tick();
    if (!body) return;
    const section = [...body.querySelectorAll<HTMLElement>('[data-tag-group]')].find(
      (el) => el.dataset.tagGroup === groupId
    );
    if (!section) return;
    const list = body.getBoundingClientRect();
    const rect = section.getBoundingClientRect();
    const offset = groupRevealOffset(
      { start: list.top, end: list.bottom },
      { start: rect.top, end: rect.bottom }
    );
    if (offset !== 0) body.scrollTop += offset;
  }

  // A group's drag, as the pointerDrag action on its handle reports it. The
  // action follows every `start` with a `drop` or a `cancel`, so no group
  // drag is ever left over.
  const groupDragHandlers: TagGroupDragHandlers = {
    scroll: { container: () => body, axis: 'y' },
    start: (groupId) => editor.groupDragStart(groupId),
    /**
     * The cursor says a drop is refused wherever it would leave the groups
     * in their order: level with the group's own section, with the half
     * where it already is, in a gap with no line, and clearly away from the
     * list.
     */
    move(point) {
      holdGroupAt(point);
      return editor.groupDropWouldMove;
    },
    /**
     * Drop the dragged group where the pointer was released, which is
     * resolved as any other point. A group that moved is then shown.
     */
    drop(point) {
      holdGroupAt(point);
      const moved = editor.groupDrop();
      if (moved !== null) void revealGroup(moved);
    },
    cancel: () => editor.groupDragEnd(),
  };
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

  <div bind:this={body} class="panel-body">
    {#if !tagsStore.loaded}
      <p class="panel-note">Loading tags…</p>
    {:else}
      {#each catalog.groups as group, i (group.id)}
        <TagGroupSection
          {editor}
          {group}
          tags={catalog.tagsByGroup.get(group.id) ?? []}
          first={i === 0}
          canMoveUp={i > 0}
          canMoveDown={i < catalog.groups.length - 1}
          groupDrag={groupDragHandlers}
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
    /* Where the list is scrolled to is its own to keep. An engine with
       scroll anchoring (WebKit has it) scrolls a list to hold still
       whatever was on screen when the content above that changes height.
       A group dropped further down leaves the content above, so the engine
       would scroll the list up by the group's height: to the top, for a
       tall group, and away from where the group landed. With anchoring off
       the list stays put, and the panel shows the group itself (see
       `revealGroup`). */
    overflow-anchor: none;
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
