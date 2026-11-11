<!--
  UngroupedTagsSection.svelte — the tags in no group, below the groups in
  Settings › Tags. A project can carry any number of these, so they can
  filter a board but can't be one. Dragging a tag here takes it out of its
  group.
-->
<script lang="ts">
  import type { Tag } from '../../../types';
  import TagList from './TagList.svelte';
  import type { TagsEditor } from './tagsEditor.svelte';

  interface Props {
    editor: TagsEditor;
    tags: Tag[];
  }

  let { editor, tags }: Props = $props();
</script>

<section class="ungrouped" aria-labelledby="tag-settings-ungrouped-heading">
  <header class="ungrouped-head">
    <h3 id="tag-settings-ungrouped-heading">Ungrouped tags</h3>
    <p>A project can have any number of these.</p>
  </header>
  <TagList
    {editor}
    groupId={null}
    {tags}
    label="Ungrouped tags"
    emptyText="No ungrouped tags. Drag a tag here to take it out of its group."
  />
</section>

<style>
  .ungrouped {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .ungrouped-head {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 2px 10px;
  }

  .ungrouped-head h3 {
    margin: 0;
    padding: 3px 0;
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  .ungrouped-head p {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }
</style>
