<!-- A context menu holding just ProjectTagMenuItems, opened from a trigger. -->
<script lang="ts">
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import type { Project } from '../../../types';
  import ProjectTagMenuItems from '../ProjectTagMenuItems.svelte';

  let { project }: { project: Project } = $props();
</script>

<ContextMenu.Root>
  <ContextMenu.Trigger>
    <div class="harness-trigger">{project.name}</div>
  </ContextMenu.Trigger>
  <ContextMenu.Content>
    <ProjectTagMenuItems {project} />
  </ContextMenu.Content>
</ContextMenu.Root>
