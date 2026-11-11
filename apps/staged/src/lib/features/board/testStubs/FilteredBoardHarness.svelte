<!--
  The board over the shared filters' result, wired as ProjectsList wires it:
  the filter chips, tag chips included, decide which projects it shows.
-->
<script lang="ts">
  import type { TagGroup } from '../../../types';
  import { projectFiltersStore } from '../../projects/projectFilters.svelte';
  import ProjectsBoard from '../ProjectsBoard.svelte';

  let { group, onOpen }: { group: TagGroup | null; onOpen: (projectId: string) => void } = $props();
</script>

<ProjectsBoard {group} projects={projectFiltersStore.filteredProjects} searchRank={null} {onOpen} />
