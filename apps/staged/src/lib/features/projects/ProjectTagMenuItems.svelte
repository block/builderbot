<!--
  ProjectTagMenuItems.svelte — a project's tags in its context menu (sidebar
  rows and project grid cards).

  One radio submenu per group, the icon group first and then group order,
  each trigger showing the project's current value; the icon group's "None"
  reads "Default", previewing the computed PR status it falls back to. Then a
  "Tags" submenu: a checkbox per ungrouped tag, "New tag…" (a small dialog
  that creates one and assigns it) and "Manage tags…" (Settings › Tags).

  On a board card (`moveToGroupId`), "Move to" comes first: the board group's
  radio submenu under the board's own column names, which is how a card
  changes column without dragging (touch, phone, keyboard). `onMoved` then
  lets the board put focus back on the card in its new column.

  Built on the ContextMenu.Sub wrappers, so on a phone each submenu is a
  sheet over the menu's own, with a Back row. Branches come from the shared
  projects store, so the Default preview reads "Loading…" rather than a guess
  until they land.
-->
<script lang="ts">
  import Settings2 from '@lucide/svelte/icons/settings-2';
  import Plus from '@lucide/svelte/icons/plus';
  import TagsIcon from '@lucide/svelte/icons/tags';
  import CircleDashed from '@lucide/svelte/icons/circle-dashed';
  import Columns3 from '@lucide/svelte/icons/columns-3';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import type { Project, TagGroup } from '../../types';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { tagsStore } from '../../stores/tags.svelte';
  import { openSettings } from '../layout/navigation.svelte';
  import TagIcon from '../tags/TagIcon.svelte';
  import { newTagDialog } from '../tags/newTagRequest.svelte';
  import { tagActions } from '../tags/tagActions.svelte';
  import { NO_TAG_MENU_VALUE } from '../tags/tagState';
  import ComputedProjectStatusIcon from './ComputedProjectStatusIcon.svelte';
  import { computedStatusLabel, resolveComputedProjectStatus } from './projectStatusDisplay';
  import { noTagColumnName } from '../board/boardColumns';

  interface Props {
    project: Project;
    /** On a board card: the board's group, offered first as "Move to". */
    moveToGroupId?: string | null;
    /** After a "Move to" pick that changed the project's column. */
    onMoved?: (projectId: string) => void;
  }

  let { project, moveToGroupId = null, onMoved }: Props = $props();

  let moveToGroup = $derived(
    moveToGroupId ? (tagsStore.catalog.groupsById.get(moveToGroupId) ?? null) : null
  );

  let groups = $derived.by(() => {
    const iconGroup = tagsStore.iconGroup;
    if (!iconGroup) return tagsStore.groups;
    return [iconGroup, ...tagsStore.groups.filter((group) => group.id !== iconGroup.id)];
  });
  let ungrouped = $derived(tagsStore.tagsByGroup.get(null) ?? []);
  let tagIds = $derived(tagsStore.tagIdsFor(project.id));
  let assignedUngrouped = $derived(ungrouped.filter((tag) => tagIds.has(tag.id)).length);
  let computed = $derived(
    resolveComputedProjectStatus(
      projectsDataStore.branchesByProject.get(project.id) ?? [],
      projectsDataStore.isProjectHydrated(project.id)
    )
  );
  let computedLabel = $derived(computedStatusLabel(computed));

  function pickGroupTag(group: TagGroup, value: string) {
    const next = value === NO_TAG_MENU_VALUE ? null : value;
    void tagActions.setProjectGroupTag(project, group.id, next);
  }

  function moveTo(group: TagGroup, value: string) {
    const next = value === NO_TAG_MENU_VALUE ? null : value;
    if ((tagsStore.groupTagFor(project.id, group.id)?.id ?? null) === next) return;
    // Optimistic: the card is in its new column as soon as this returns.
    void tagActions.setProjectGroupTag(project, group.id, next);
    onMoved?.(project.id);
  }
</script>

{#if moveToGroup}
  {@const current = tagsStore.groupTagFor(project.id, moveToGroup.id)}
  {@const tags = tagsStore.tagsByGroup.get(moveToGroup.id) ?? []}
  <ContextMenu.Sub>
    <ContextMenu.SubTrigger data-tag-group-trigger="move-to">
      <Columns3 size={14} />
      <span class="min-w-0 flex-1 truncate">Move to</span>
    </ContextMenu.SubTrigger>
    <ContextMenu.SubContent class="min-w-[200px]">
      <ContextMenu.RadioGroup
        value={current?.id ?? NO_TAG_MENU_VALUE}
        onValueChange={(value) => moveTo(moveToGroup, value)}
      >
        <ContextMenu.RadioItem value={NO_TAG_MENU_VALUE}>
          <span class="truncate">{noTagColumnName(moveToGroup)}</span>
        </ContextMenu.RadioItem>
        {#each tags as tag (tag.id)}
          <ContextMenu.RadioItem value={tag.id}>
            <TagIcon {tag} size={14} />
            <span class="truncate">{tag.name}</span>
          </ContextMenu.RadioItem>
        {/each}
      </ContextMenu.RadioGroup>
    </ContextMenu.SubContent>
  </ContextMenu.Sub>
  <ContextMenu.Separator />
{/if}

{#each groups as group (group.id)}
  {@const current = tagsStore.groupTagFor(project.id, group.id)}
  {@const tags = tagsStore.tagsByGroup.get(group.id) ?? []}
  <ContextMenu.Sub>
    <ContextMenu.SubTrigger data-tag-group-trigger={group.id}>
      {#if current}
        <TagIcon tag={current} size={14} />
      {:else if group.isIconGroup}
        <ComputedProjectStatusIcon status={computed} size={14} />
      {:else}
        <CircleDashed size={14} class="text-muted-foreground" />
      {/if}
      <span class="min-w-0 flex-1 truncate">{group.name}</span>
      <span class="max-w-[120px] truncate pl-3 text-xs text-muted-foreground">
        {current?.name ?? (group.isIconGroup ? computedLabel : 'None')}
      </span>
    </ContextMenu.SubTrigger>
    <ContextMenu.SubContent class="min-w-[200px]">
      <ContextMenu.RadioGroup
        value={current?.id ?? NO_TAG_MENU_VALUE}
        onValueChange={(value) => pickGroupTag(group, value)}
      >
        {#if group.isIconGroup}
          <ContextMenu.RadioItem value={NO_TAG_MENU_VALUE}>
            <ComputedProjectStatusIcon status={computed} size={14} />
            <span class="flex min-w-0 flex-col">
              <span>Default</span>
              <span class="truncate text-xs text-muted-foreground">{computedLabel}</span>
            </span>
          </ContextMenu.RadioItem>
        {:else}
          <ContextMenu.RadioItem value={NO_TAG_MENU_VALUE}>
            <span class="text-muted-foreground">None</span>
          </ContextMenu.RadioItem>
        {/if}
        {#if tags.length > 0}
          <ContextMenu.Separator />
          {#each tags as tag (tag.id)}
            <ContextMenu.RadioItem value={tag.id}>
              <TagIcon {tag} size={14} />
              <span class="truncate">{tag.name}</span>
            </ContextMenu.RadioItem>
          {/each}
        {/if}
      </ContextMenu.RadioGroup>
      <ContextMenu.Separator />
      <ContextMenu.Item onSelect={() => openSettings('tags')}>
        <Settings2 size={14} />
        Edit tags…
      </ContextMenu.Item>
    </ContextMenu.SubContent>
  </ContextMenu.Sub>
{/each}

<ContextMenu.Sub>
  <ContextMenu.SubTrigger data-tag-group-trigger="ungrouped">
    <TagsIcon size={14} />
    <span class="min-w-0 flex-1 truncate">Tags</span>
    {#if assignedUngrouped > 0}
      <span class="pl-3 text-xs text-muted-foreground">{assignedUngrouped}</span>
    {/if}
  </ContextMenu.SubTrigger>
  <ContextMenu.SubContent class="min-w-[200px]">
    {#each ungrouped as tag (tag.id)}
      <!-- Stays open, so several tags can be ticked in one go. -->
      <ContextMenu.CheckboxItem
        checked={tagIds.has(tag.id)}
        closeOnSelect={false}
        onCheckedChange={(checked) =>
          void tagActions.setProjectTagAssigned(project, tag.id, checked)}
      >
        <TagIcon {tag} size={14} />
        <span class="truncate">{tag.name}</span>
      </ContextMenu.CheckboxItem>
    {/each}
    {#if ungrouped.length > 0}
      <ContextMenu.Separator />
    {/if}
    <ContextMenu.Item onSelect={() => newTagDialog.request(project)}>
      <Plus size={14} />
      New tag…
    </ContextMenu.Item>
    <ContextMenu.Item onSelect={() => openSettings('tags')}>
      <Settings2 size={14} />
      Manage tags…
    </ContextMenu.Item>
  </ContextMenu.SubContent>
</ContextMenu.Sub>
