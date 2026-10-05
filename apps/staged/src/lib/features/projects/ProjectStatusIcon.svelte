<!--
  ProjectStatusIcon.svelte — the one icon a project leads with: its chosen
  status option, or else its computed status (aggregate PR state for local
  projects, workspace state for remote ones). See projectStatusDisplay.ts for
  the precedence rules.

  Shared by the sidebar rows, the project grid cards and the detail-page
  status picker so a project reads the same everywhere. Renders the bare SVG
  (or a same-sized placeholder) so parents keep control of layout.
-->
<script lang="ts">
  import type { Branch, Project } from '../../types';
  import { preferences } from '../settings/preferences.svelte';
  import { resolveProjectStatus } from './projectStatusDisplay';
  import ComputedProjectStatusIcon from './ComputedProjectStatusIcon.svelte';
  import ProjectStatusOptionIcon from './ProjectStatusOptionIcon.svelte';

  interface Props {
    project: Project;
    branches: Branch[];
    /** False while the project's branches are still loading. */
    hydrated?: boolean;
    size?: number;
  }

  let { project, branches, hydrated = true, size = 14 }: Props = $props();

  let status = $derived(
    resolveProjectStatus(project, branches, preferences.projectStatusOptions, hydrated)
  );
</script>

{#if status.kind === 'override'}
  <ProjectStatusOptionIcon option={status.option} {size} />
{:else}
  <ComputedProjectStatusIcon {status} {size} />
{/if}
