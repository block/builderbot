<!--
  ProjectStatusPicker.svelte — the status pill beside a project's title.

  Acts on the icon tag group (the imported Status group, unless the user
  picks another) and renders nothing when there isn't one. The pill shows
  what the project's status icon shows everywhere else (see
  ProjectStatusIcon) and opens a menu to pick one of the group's tags, or
  Default to go back to the computed PR status. The Default row previews
  that computed status, so the user sees what they'd fall back to. The
  group's tags are edited in Settings > General. The sidebar and grid
  context menus offer the same choices through ProjectStatusSubmenu.
-->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Settings2 from '@lucide/svelte/icons/settings-2';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import type { Branch, Project } from '../../types';
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
    branches: Branch[];
    disabled?: boolean;
  }

  let { project, branches, disabled = false }: Props = $props();

  let group = $derived(tagsStore.iconGroup);
  let tags = $derived(group ? (tagsStore.tagsByGroup.get(group.id) ?? []) : []);
  let current = $derived(group ? tagsStore.groupTagFor(project.id, group.id) : null);
  let computed = $derived(resolveComputedProjectStatus(branches));
  let computedLabel = $derived(computedStatusLabel(computed));

  function handleValueChange(value: string) {
    if (!group) return;
    const next = value === NO_TAG_MENU_VALUE ? null : value;
    void tagActions.setProjectGroupTag(project, group.id, next);
  }
</script>

{#if group}
  <DropdownMenu.Root>
    <DropdownMenu.Trigger
      class="project-status-pill touch-hit"
      {disabled}
      title={`Set project ${group.name.toLowerCase()}`}
      style={current ? `color: ${TAG_COLOR_VARS[current.color]}` : undefined}
    >
      {#if current}
        <TagIcon tag={current} size={12} />
        <span class="pill-label">{current.name}</span>
      {:else}
        <ComputedProjectStatusIcon status={computed} size={12} />
        <span class="pill-label">{computedLabel}</span>
      {/if}
      <ChevronDown size={12} class="pill-chevron" />
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="end" sideOffset={4} class="min-w-[200px]">
      <DropdownMenu.RadioGroup
        value={current?.id ?? NO_TAG_MENU_VALUE}
        onValueChange={handleValueChange}
      >
        <DropdownMenu.RadioItem value={NO_TAG_MENU_VALUE}>
          <ComputedProjectStatusIcon status={computed} size={14} />
          <span class="flex min-w-0 flex-col">
            <span>Default</span>
            <span class="truncate text-xs text-muted-foreground">{computedLabel}</span>
          </span>
        </DropdownMenu.RadioItem>
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
      <DropdownMenu.Item onSelect={() => openSettings('general')}>
        <Settings2 size={14} />
        Edit statuses…
      </DropdownMenu.Item>
    </DropdownMenu.Content>
  </DropdownMenu.Root>
{/if}

<style>
  :global(.project-status-pill) {
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

  :global(.project-status-pill:hover:not(:disabled)) {
    background: var(--bg-hover);
    border-color: var(--border-emphasis);
  }

  :global(.project-status-pill:disabled) {
    cursor: default;
    opacity: 0.6;
  }

  :global(.project-status-pill svg) {
    flex-shrink: 0;
  }

  .pill-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  :global(.project-status-pill .pill-chevron) {
    color: var(--text-muted);
  }
</style>
