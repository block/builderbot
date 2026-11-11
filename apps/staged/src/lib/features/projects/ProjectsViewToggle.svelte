<!--
  ProjectsViewToggle.svelte — the home page's Grid | Board segmented control.

  bits-ui clears a single-select toggle group when its pressed item is clicked
  again. With a one-way `value` that '' would stick in the shadcn wrapper and
  leave no segment pressed, so the value goes through a function binding (the
  ProjectLocationPicker pattern): the setter drops the clear and the getter
  hands the current mode back, and the parent only hears about real changes.
-->
<script lang="ts">
  import Columns3 from '@lucide/svelte/icons/columns-3';
  import LayoutGrid from '@lucide/svelte/icons/layout-grid';
  import * as ToggleGroup from '$lib/components/ui/toggle-group';
  import type { ProjectsViewMode } from '../board/boardPrefs';

  interface Props {
    value: ProjectsViewMode;
    onChange: (mode: ProjectsViewMode) => void;
  }

  let { value, onChange }: Props = $props();
</script>

<ToggleGroup.Root
  type="single"
  variant="outline"
  size="sm"
  bind:value={
    () => value,
    (next) => {
      if ((next === 'grid' || next === 'board') && next !== value) onChange(next);
    }
  }
  class="view-toggle w-fit shrink-0"
  aria-label="Projects view"
>
  <ToggleGroup.Item value="grid" aria-label="Grid" title="Grid">
    <LayoutGrid size={14} />
    <span class="view-toggle-label">Grid</span>
  </ToggleGroup.Item>
  <ToggleGroup.Item value="board" aria-label="Board" title="Board">
    <Columns3 size={14} />
    <span class="view-toggle-label">Board</span>
  </ToggleGroup.Item>
</ToggleGroup.Root>

<style>
  /* Icon-only on a phone, where the search field needs the room. */
  @media (max-width: 640px) {
    .view-toggle-label {
      display: none;
    }
  }
</style>
