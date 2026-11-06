<!--
  Test stub for BranchTimeline: renders only its "adding..." rows (pendingAdds)
  as plain elements, so component tests can tell when the card shows its
  timeline and watch a drop's or pick's rows come and go without the real
  timeline's rows and menus.
-->
<script lang="ts">
  import type { PendingAdd } from '../../branches/timelineImages';

  let { pendingAdds = [] }: { pendingAdds?: PendingAdd[] } = $props();
</script>

<div data-testid="branch-timeline">
  {#each pendingAdds as add (add.key)}
    <div data-testid="pending-add" data-type={add.type} data-title={add.title}></div>
  {/each}
</div>
