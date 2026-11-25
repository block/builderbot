<!--
  Test stub for NoteTransferMenu: a marker that reports which note the menu
  was rendered for and whether its "Move" would be disabled, so a test can
  assert when a note dialog offers the menu without mounting the picker.
-->
<script lang="ts">
  let {
    noteId,
    sessionActive = false,
  }: {
    noteId: string;
    sessionActive?: boolean;
  } = $props();
</script>

<span
  data-testid="note-transfer-menu"
  data-note-id={noteId}
  data-session-active={sessionActive ? '' : undefined}
></span>
