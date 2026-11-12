<!--
  Test stub for BranchTimeline: renders only its "adding..." rows (pendingAdds)
  as plain elements, so component tests can tell when the card shows its
  timeline and watch a drop's or pick's rows come and go without the real
  timeline's rows and menus. A button stands in for clicking a note row, so a
  test can open the card's note dialog.
-->
<script lang="ts">
  import type { PendingAdd } from '../../branches/timelineImages';
  import type { NoteClickInfo } from '../noteFreshness';

  let {
    pendingAdds = [],
    onNoteClick,
  }: { pendingAdds?: PendingAdd[]; onNoteClick?: (note: NoteClickInfo) => void } = $props();
</script>

<div data-testid="branch-timeline">
  <button
    type="button"
    data-testid="open-note"
    onclick={() =>
      onNoteClick?.({
        noteId: 'note-1',
        title: 'Plan',
        content: 'Body',
        sessionId: 'note-session',
        updatedAt: 1,
      })}>Plan</button
  >
  {#each pendingAdds as add (add.key)}
    <div data-testid="pending-add" data-type={add.type} data-title={add.title}></div>
  {/each}
</div>
