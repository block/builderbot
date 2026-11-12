<!--
  ProjectStatusIcon.svelte — the one icon a project leads with: its tag in the
  icon group, or else its computed status (aggregate PR state), whether the
  project is local or remote. See projectStatusDisplay.ts for the precedence
  rules.

  Shared by the sidebar rows and the project grid cards (the header's icon
  group pill resolves the same way) so a project reads the same everywhere. Renders the bare SVG
  (or a same-sized placeholder) so parents keep control of layout.
-->
<script lang="ts">
  import type { Branch, Project } from '../../types';
  import { tagsStore } from '../../stores/tags.svelte';
  import TagIcon from '../tags/TagIcon.svelte';
  import { resolveProjectStatus } from './projectStatusDisplay';
  import ComputedProjectStatusIcon from './ComputedProjectStatusIcon.svelte';

  interface Props {
    project: Project;
    branches: Branch[];
    /** False while the project's branches are still loading. */
    hydrated?: boolean;
    size?: number;
  }

  let { project, branches, hydrated = true, size = 14 }: Props = $props();

  let status = $derived(
    resolveProjectStatus(tagsStore.iconTagFor(project.id), branches, hydrated, tagsStore.loaded)
  );
</script>

{#if status.kind === 'tag'}
  <TagIcon tag={status.tag} {size} />
{:else}
  <ComputedProjectStatusIcon {status} {size} />
{/if}
