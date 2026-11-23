<!-- A timeline of real rows inside one TimelineContextMenu, as BranchTimeline
     lays them out: one trigger around the whole list. A row with `menu: false`
     has no context-menu key and no action. -->
<script lang="ts">
  import TimelineContextMenu from '../TimelineContextMenu.svelte';
  import TimelineRow from '../TimelineRow.svelte';

  export type TimelineWithMenuRow = { key: string; titleHtml: string; menu?: boolean };

  let {
    rows,
    onDelete,
  }: {
    rows: TimelineWithMenuRow[];
    onDelete: (key: string) => void;
  } = $props();

  let actions = $derived(
    rows
      .filter((row) => row.menu !== false)
      .map((row) => ({ key: row.key, onDelete: () => onDelete(row.key) }))
  );
</script>

<TimelineContextMenu {actions}>
  <div class="timeline">
    {#each rows as row (row.key)}
      <div>
        <TimelineRow
          type="note"
          title={row.key}
          titleHtml={row.titleHtml}
          contextMenuKey={row.menu === false ? undefined : row.key}
        />
      </div>
    {/each}
  </div>
</TimelineContextMenu>
