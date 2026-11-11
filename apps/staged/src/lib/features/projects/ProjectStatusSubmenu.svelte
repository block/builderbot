<!--
  ProjectStatusSubmenu.svelte — the "Set Status" submenu in a project's
  context menu (sidebar rows and project grid cards).

  Offers the same choices as the detail page's ProjectStatusPicker, over the
  icon tag group: Default, previewing the computed PR status, each of the
  group's tags, and "Edit statuses…". Renders nothing when there's no icon
  group. Branches come from the shared projects store, so the Default preview
  reads "Loading…" rather than a guess until they land.
-->
<script lang="ts">
  import Flag from '@lucide/svelte/icons/flag';
  import Settings2 from '@lucide/svelte/icons/settings-2';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import type { Project } from '../../types';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { tagsStore } from '../../stores/tags.svelte';
  import { openSettings } from '../layout/navigation.svelte';
  import TagIcon from '../tags/TagIcon.svelte';
  import { tagActions } from '../tags/tagActions.svelte';
  import { NO_TAG_MENU_VALUE } from '../tags/tagState';
  import ComputedProjectStatusIcon from './ComputedProjectStatusIcon.svelte';
  import { computedStatusLabel, resolveComputedProjectStatus } from './projectStatusDisplay';

  interface Props {
    project: Project;
  }

  let { project }: Props = $props();

  let group = $derived(tagsStore.iconGroup);
  let tags = $derived(group ? (tagsStore.tagsByGroup.get(group.id) ?? []) : []);
  let current = $derived(group ? tagsStore.groupTagFor(project.id, group.id) : null);
  let computed = $derived(
    resolveComputedProjectStatus(
      projectsDataStore.branchesByProject.get(project.id) ?? [],
      projectsDataStore.isProjectHydrated(project.id)
    )
  );
  let computedLabel = $derived(computedStatusLabel(computed));

  function handleValueChange(value: string) {
    if (!group) return;
    const next = value === NO_TAG_MENU_VALUE ? null : value;
    void tagActions.setProjectGroupTag(project, group.id, next);
  }
</script>

{#if group}
  <ContextMenu.Sub>
    <ContextMenu.SubTrigger>
      <Flag size={14} /> Set {group.name}
    </ContextMenu.SubTrigger>
    <ContextMenu.SubContent class="min-w-[200px]">
      <ContextMenu.RadioGroup
        value={current?.id ?? NO_TAG_MENU_VALUE}
        onValueChange={handleValueChange}
      >
        <ContextMenu.RadioItem value={NO_TAG_MENU_VALUE}>
          <ComputedProjectStatusIcon status={computed} size={14} />
          <span class="flex min-w-0 flex-col">
            <span>Default</span>
            <span class="truncate text-xs text-muted-foreground">{computedLabel}</span>
          </span>
        </ContextMenu.RadioItem>
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
      <ContextMenu.Item onSelect={() => openSettings('general')}>
        <Settings2 size={14} />
        Edit statuses…
      </ContextMenu.Item>
    </ContextMenu.SubContent>
  </ContextMenu.Sub>
{/if}
