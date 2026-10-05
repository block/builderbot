<!--
  ProjectStatusSubmenu.svelte — the "Set Status" submenu in a project's
  context menu (sidebar rows and project grid cards).

  Offers the same choices as the detail page's ProjectStatusPicker: Default,
  previewing the computed PR/cloud status, each configured status option, and
  "Edit statuses…". Branches come from the shared projects store, so the
  Default preview reads "Loading…" rather than a guess until they land.
-->
<script lang="ts">
  import Flag from '@lucide/svelte/icons/flag';
  import Settings2 from '@lucide/svelte/icons/settings-2';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import type { Project } from '../../types';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { openSettings } from '../layout/navigation.svelte';
  import { preferences } from '../settings/preferences.svelte';
  import ComputedProjectStatusIcon from './ComputedProjectStatusIcon.svelte';
  import ProjectStatusOptionIcon from './ProjectStatusOptionIcon.svelte';
  import { projectActions } from './projectActions.svelte';
  import {
    DEFAULT_STATUS_MENU_VALUE,
    computedStatusLabel,
    findProjectStatusOverride,
    projectStatusOptionLabel,
    resolveComputedProjectStatus,
  } from './projectStatusDisplay';

  interface Props {
    project: Project;
    disabled?: boolean;
  }

  let { project, disabled = false }: Props = $props();

  let options = $derived(preferences.projectStatusOptions);
  let override = $derived(findProjectStatusOverride(project, options));
  let computed = $derived(
    resolveComputedProjectStatus(
      project,
      projectsDataStore.branchesByProject.get(project.id) ?? [],
      projectsDataStore.isProjectHydrated(project.id)
    )
  );
  let computedLabel = $derived(computedStatusLabel(computed));

  function handleValueChange(value: string) {
    const next = value === DEFAULT_STATUS_MENU_VALUE ? null : value;
    void projectActions.setProjectStatusOverride(project, next);
  }
</script>

<ContextMenu.Sub>
  <ContextMenu.SubTrigger {disabled}>
    <Flag size={14} /> Set Status
  </ContextMenu.SubTrigger>
  <ContextMenu.SubContent class="min-w-[200px]">
    <ContextMenu.RadioGroup
      value={override?.id ?? DEFAULT_STATUS_MENU_VALUE}
      onValueChange={handleValueChange}
    >
      <ContextMenu.RadioItem value={DEFAULT_STATUS_MENU_VALUE}>
        <ComputedProjectStatusIcon status={computed} size={14} />
        <span class="flex min-w-0 flex-col">
          <span>Default</span>
          <span class="truncate text-xs text-muted-foreground">{computedLabel}</span>
        </span>
      </ContextMenu.RadioItem>
      {#if options.length > 0}
        <ContextMenu.Separator />
        {#each options as option (option.id)}
          <ContextMenu.RadioItem value={option.id}>
            <ProjectStatusOptionIcon {option} size={14} />
            <span class="truncate">{projectStatusOptionLabel(option)}</span>
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
