<!--
  TagGroupPill.svelte — one tag group's pill in a project's header (see
  ProjectTagPills): the project's tag in that group, opening a menu to switch
  it in one click.

  The menu is a radio list: "None", then the group's tags, then "Edit tags…"
  (Settings › Tags). An empty value shows as a dim pill with the group's name.
  The icon group is the exception: with no tag the project shows its computed
  PR status (as its status icon does everywhere else), so the pill shows that,
  and its "None" row reads "Default" with a preview of the computed status.
-->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Settings2 from '@lucide/svelte/icons/settings-2';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import type { Branch, Project, TagGroup } from '../../types';
  import { tagsStore } from '../../stores/tags.svelte';
  import { openSettings } from '../layout/navigation.svelte';
  import TagIcon from '../tags/TagIcon.svelte';
  import { tagActions } from '../tags/tagActions.svelte';
  import { TAG_COLOR_VARS } from '../tags/tagColors';
  import { NO_TAG_MENU_VALUE } from '../tags/tagState';
  import ComputedProjectStatusIcon from './ComputedProjectStatusIcon.svelte';
  import { computedStatusLabel, resolveComputedProjectStatus } from './projectStatusDisplay';

  interface Props {
    project: Project;
    group: TagGroup;
    branches: Branch[];
    disabled?: boolean;
  }

  let { project, group, branches, disabled = false }: Props = $props();

  let tags = $derived(tagsStore.tagsByGroup.get(group.id) ?? []);
  let current = $derived(tagsStore.groupTagFor(project.id, group.id));
  let computed = $derived(resolveComputedProjectStatus(branches));
  let computedLabel = $derived(computedStatusLabel(computed));
  let empty = $derived(!current && !group.isIconGroup);

  function handleValueChange(value: string) {
    const next = value === NO_TAG_MENU_VALUE ? null : value;
    void tagActions.setProjectGroupTag(project, group.id, next);
  }
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger
    class={empty ? 'tag-group-pill touch-hit empty' : 'tag-group-pill touch-hit'}
    {disabled}
    title={`Set ${group.name}`}
    aria-label={`${group.name}: ${current?.name ?? (group.isIconGroup ? computedLabel : 'None')}`}
    style={current ? `color: ${TAG_COLOR_VARS[current.color]}` : undefined}
  >
    {#if current}
      <TagIcon tag={current} size={12} />
      <span class="pill-label">{current.name}</span>
    {:else if group.isIconGroup}
      <ComputedProjectStatusIcon status={computed} size={12} />
      <span class="pill-label">{computedLabel}</span>
    {:else}
      <span class="pill-label">{group.name}</span>
    {/if}
    <ChevronDown size={12} class="pill-chevron" />
  </DropdownMenu.Trigger>
  <DropdownMenu.Content align="start" sideOffset={4} class="min-w-[200px]">
    <DropdownMenu.Label>{group.name}</DropdownMenu.Label>
    <DropdownMenu.RadioGroup
      value={current?.id ?? NO_TAG_MENU_VALUE}
      onValueChange={handleValueChange}
    >
      {#if group.isIconGroup}
        <DropdownMenu.RadioItem value={NO_TAG_MENU_VALUE}>
          <ComputedProjectStatusIcon status={computed} size={14} />
          <span class="flex min-w-0 flex-col">
            <span>Default</span>
            <span class="truncate text-xs text-muted-foreground">{computedLabel}</span>
          </span>
        </DropdownMenu.RadioItem>
      {:else}
        <DropdownMenu.RadioItem value={NO_TAG_MENU_VALUE}>
          <span class="text-muted-foreground">None</span>
        </DropdownMenu.RadioItem>
      {/if}
      {#if tags.length > 0}
        <DropdownMenu.Separator />
        {#each tags as tag (tag.id)}
          <DropdownMenu.RadioItem value={tag.id}>
            <TagIcon {tag} size={14} />
            <span class="truncate">{tag.name}</span>
          </DropdownMenu.RadioItem>
        {/each}
      {/if}
    </DropdownMenu.RadioGroup>
    <DropdownMenu.Separator />
    <DropdownMenu.Item onSelect={() => openSettings('tags')}>
      <Settings2 size={14} />
      Edit tags…
    </DropdownMenu.Item>
  </DropdownMenu.Content>
</DropdownMenu.Root>

<style>
  :global(.tag-group-pill) {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex-shrink: 0;
    max-width: 200px;
    padding: 2px 6px 2px 8px;
    border: 1px solid var(--border-muted);
    border-radius: 999px;
    background: transparent;
    color: var(--text-secondary);
    font-size: var(--size-xs);
    font-weight: 600;
    line-height: 1.4;
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease;
  }

  :global(.tag-group-pill.empty) {
    border-style: dashed;
    color: var(--text-faint);
    font-weight: 500;
  }

  :global(.tag-group-pill:hover:not(:disabled)) {
    background: var(--bg-hover);
    border-color: var(--border-emphasis);
  }

  :global(.tag-group-pill.empty:hover:not(:disabled)) {
    color: var(--text-muted);
  }

  :global(.tag-group-pill:disabled) {
    cursor: default;
    opacity: 0.6;
  }

  :global(.tag-group-pill svg) {
    flex-shrink: 0;
  }

  .pill-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  :global(.tag-group-pill .pill-chevron) {
    color: var(--text-muted);
  }
</style>
