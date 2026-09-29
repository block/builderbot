<!--
  ProjectStatusPicker.svelte — the status pill beside a project's title.

  The pill shows what the project's status icon shows everywhere else (see
  ProjectStatusIcon) and opens a menu to pick one of the configured status
  options, or Default to go back to the computed PR/cloud status. The Default
  row previews that computed status, so the user sees what they'd fall back
  to. The option list itself is edited in Settings > General.
-->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Settings2 from '@lucide/svelte/icons/settings-2';
  import { toast } from 'svelte-sonner';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import * as commands from '../../api/commands';
  import type { Branch, Project } from '../../types';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { openSettings } from '../layout/navigation.svelte';
  import { preferences } from '../settings/preferences.svelte';
  import ComputedProjectStatusIcon from './ComputedProjectStatusIcon.svelte';
  import ProjectStatusOptionIcon from './ProjectStatusOptionIcon.svelte';
  import {
    PROJECT_STATUS_COLOR_VARS,
    computedStatusLabel,
    findProjectStatusOverride,
    projectStatusOptionLabel,
    resolveComputedProjectStatus,
  } from './projectStatusDisplay';

  /** Radio value for Default; option ids are slugs or UUIDs, so it can't clash. */
  const DEFAULT_VALUE = '__default__';

  interface Props {
    project: Project;
    branches: Branch[];
    disabled?: boolean;
  }

  let { project, branches, disabled = false }: Props = $props();

  let options = $derived(preferences.projectStatusOptions);
  let override = $derived(findProjectStatusOverride(project, options));
  let computed = $derived(resolveComputedProjectStatus(project, branches));
  let computedLabel = $derived(computedStatusLabel(computed));

  async function handleValueChange(value: string) {
    const next = value === DEFAULT_VALUE ? null : value;
    // A dangling id already reads as Default; choosing Default clears it too.
    if (next === project.statusOverride) return;

    const projectId = project.id;
    const previous = project.statusOverride;
    projectsDataStore.projectStatusOverrideChanged(projectId, next);
    try {
      await commands.setProjectStatusOverride(projectId, next);
    } catch (e) {
      projectsDataStore.projectStatusOverrideChanged(projectId, previous);
      toast.error('Unable to set project status', {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  }
</script>

<DropdownMenu.Root>
  <DropdownMenu.Trigger
    class="project-status-pill"
    {disabled}
    title="Set project status"
    style={override ? `color: ${PROJECT_STATUS_COLOR_VARS[override.color]}` : undefined}
  >
    {#if override}
      <ProjectStatusOptionIcon option={override} size={12} />
      <span class="pill-label">{projectStatusOptionLabel(override)}</span>
    {:else}
      <ComputedProjectStatusIcon status={computed} size={12} />
      <span class="pill-label">{computedLabel}</span>
    {/if}
    <ChevronDown size={12} class="pill-chevron" />
  </DropdownMenu.Trigger>
  <DropdownMenu.Content align="start" sideOffset={4} class="min-w-[200px]">
    <DropdownMenu.RadioGroup
      value={override?.id ?? DEFAULT_VALUE}
      onValueChange={(value) => void handleValueChange(value)}
    >
      <DropdownMenu.RadioItem value={DEFAULT_VALUE}>
        <ComputedProjectStatusIcon status={computed} size={14} />
        <span class="flex min-w-0 flex-col">
          <span>Default</span>
          <span class="truncate text-xs text-muted-foreground">{computedLabel}</span>
        </span>
      </DropdownMenu.RadioItem>
      {#if options.length > 0}
        <DropdownMenu.Separator />
        {#each options as option (option.id)}
          <DropdownMenu.RadioItem value={option.id}>
            <ProjectStatusOptionIcon {option} size={14} />
            <span class="truncate">{projectStatusOptionLabel(option)}</span>
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
