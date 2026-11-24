<!--
  One row of `LiveSessionRowsView`. `onRender` is called from an effect that
  makes the same two reads as the markup, so it counts the times the row's
  markup is evaluated again.
-->
<script lang="ts">
  import type { LiveSessionRows } from '../liveSessionHints';

  let {
    rows,
    sessionId,
    onRender,
  }: {
    rows: LiveSessionRows;
    sessionId: string;
    onRender: (sessionId: string) => void;
  } = $props();

  $effect.pre(() => {
    void [rows.title(sessionId), rows.hint(sessionId)];
    onRender(sessionId);
  });
</script>

<li data-session={sessionId}>
  <span data-part="title">{rows.title(sessionId) ?? ''}</span>
  <span data-part="hint">{rows.hint(sessionId) ?? 'Generating note'}</span>
</li>
