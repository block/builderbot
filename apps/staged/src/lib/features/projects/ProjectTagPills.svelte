<!--
  ProjectTagPills.svelte — the row of tags under a project's title.

  One pill per tag group, the icon group first then group order (as the
  context menus and the board order them), each switching the project's tag
  in that group in one click (TagGroupPill); then the project's ungrouped tags as
  removable chips; then `+ Tag`, the searchable picker that can also create a
  tag (TagPicker). The T shortcut opens that picker: ProjectHome bumps
  `pickerRequest`, and each bump after mount opens it once.

  Hidden until the tags have loaded, so the row doesn't paint empty first.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import type { Branch, Project, Tag } from '../../types';
  import { tagsStore } from '../../stores/tags.svelte';
  import TagChip from '../tags/TagChip.svelte';
  import TagPicker from '../tags/TagPicker.svelte';
  import { tagActions } from '../tags/tagActions.svelte';
  import { iconGroupFirst } from '../tags/tagState';
  import TagGroupPill from './TagGroupPill.svelte';

  interface Props {
    project: Project;
    branches: Branch[];
    disabled?: boolean;
    /** Bumped to open the tag picker (the T shortcut). */
    pickerRequest?: number;
  }

  let { project, branches, disabled = false, pickerRequest = 0 }: Props = $props();

  let pickerOpen = $state(false);
  let handledRequest = untrack(() => pickerRequest);

  let groups = $derived(iconGroupFirst(tagsStore.groups, tagsStore.iconGroup));
  let tagIds = $derived(tagsStore.tagIdsFor(project.id));
  let ungrouped = $derived((tagsStore.tagsByGroup.get(null) ?? []).filter((t) => tagIds.has(t.id)));

  $effect(() => {
    if (pickerRequest === handledRequest) return;
    handledRequest = pickerRequest;
    if (!untrack(() => disabled)) pickerOpen = true;
  });

  function removeTag(tag: Tag) {
    void tagActions.setProjectTagAssigned(project, tag.id, false);
  }
</script>

{#if tagsStore.loaded}
  <div class="project-tag-pills" role="group" aria-label="Project tags">
    {#each groups as group (group.id)}
      <TagGroupPill {project} {group} {branches} {disabled} />
    {/each}
    {#each ungrouped as tag (tag.id)}
      <TagChip {tag} {disabled} onRemove={() => removeTag(tag)} />
    {/each}
    <TagPicker {project} {disabled} bind:open={pickerOpen} />
  </div>
{/if}

<style>
  .project-tag-pills {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
</style>
