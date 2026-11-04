<!--
  ProjectLocationPicker.svelte — the Local / Remote choice for a new project.

  Shared by NewProjectForm and the note transfer picker's "create project"
  path. Hosts only render it when remote projects are available at all (see
  `sqState`); `remoteDisabledReason` is for a host that has remote but can't
  use it for this project. It disables the Remote tile and is shown as a
  helper line below the tiles rather than inside the faded tile. The group
  and the tile are both described by it, since a disabled tile takes no focus.
  A second click on the chosen tile would clear a single-select toggle group,
  so that click is ignored: a project always has a location.
-->
<script lang="ts">
  import Cloud from '@lucide/svelte/icons/cloud';
  import Monitor from '@lucide/svelte/icons/monitor';
  import * as ToggleGroup from '$lib/components/ui/toggle-group';
  import { cn } from '$lib/components/utils.js';
  import type { ProjectLocation } from '../../types';

  interface Props {
    value: ProjectLocation;
    disabled?: boolean;
    /** Why remote can't be picked here; disables that option and says why below it. */
    remoteDisabledReason?: string | null;
    /** Stacked tiles, or side by side where height is short. */
    orientation?: 'horizontal' | 'vertical';
  }

  let {
    value = $bindable('local'),
    disabled = false,
    remoteDisabledReason = null,
    orientation = 'vertical',
  }: Props = $props();

  const reasonId = $props.id();
  let describedBy = $derived(remoteDisabledReason ? reasonId : undefined);

  let options = $derived([
    {
      value: 'local',
      label: 'Local',
      description: 'Run agents on your machine',
      icon: Monitor,
      disabled: false,
    },
    {
      value: 'remote',
      label: 'Remote',
      description: 'Run agents in the cloud',
      icon: Cloud,
      disabled: !!remoteDisabledReason,
    },
  ] satisfies {
    value: ProjectLocation;
    label: string;
    description: string;
    icon: typeof Monitor;
    disabled: boolean;
  }[]);
</script>

<ToggleGroup.Root
  type="single"
  {orientation}
  bind:value={() => value, (next) => next && (value = next as ProjectLocation)}
  {disabled}
  spacing={2}
  aria-label="Project location"
  aria-describedby={describedBy}
  class={cn('w-full', orientation === 'horizontal' && 'grid grid-cols-2 items-stretch')}
>
  {#each options as option (option.value)}
    <ToggleGroup.Item
      value={option.value}
      disabled={option.disabled}
      aria-describedby={option.disabled ? describedBy : undefined}
      class="flex h-auto w-full min-w-0 items-center justify-start gap-3 whitespace-normal rounded-[10px] border-[1.5px] border-border bg-background px-4 py-3.5 text-left text-muted-foreground hover:border-ring hover:bg-background hover:text-foreground data-[state=on]:border-foreground data-[state=on]:bg-foreground data-[state=on]:text-background"
    >
      <option.icon size={22} class="shrink-0" />
      <span class="flex min-w-0 flex-col gap-0.5">
        <span class="text-sm font-medium">{option.label}</span>
        <span class="text-xs opacity-70">{option.description}</span>
      </span>
    </ToggleGroup.Item>
  {/each}
</ToggleGroup.Root>
{#if remoteDisabledReason}
  <p id={reasonId} class="text-xs text-muted-foreground">{remoteDisabledReason}</p>
{/if}
