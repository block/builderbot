<!--
  Test stub for TimelineRow: records the row state a parent hands it (type,
  title, subtitle, session) as data attributes and exposes its Resume and
  Delete callbacks as plain buttons, so component tests can assert on and
  drive a list's rows without the real row's markup.
-->
<script lang="ts">
  let {
    type,
    title,
    secondaryMeta,
    sessionId,
    deleting = false,
    onResumeClick,
    onDeleteClick,
  }: {
    type: string;
    title: string;
    secondaryMeta?: string;
    sessionId?: string;
    deleting?: boolean;
    onResumeClick?: () => void;
    onDeleteClick?: (opts?: { altKey: boolean }) => void;
  } = $props();
</script>

<div
  data-testid="timeline-row"
  data-type={type}
  data-title={title}
  data-secondary-meta={secondaryMeta ?? ''}
  data-session-id={sessionId ?? ''}
  data-deleting={deleting}
>
  {#if onResumeClick}
    <button type="button" data-testid="row-resume" onclick={() => onResumeClick()}>Resume</button>
  {/if}
  {#if onDeleteClick}
    <button
      type="button"
      data-testid="row-delete"
      onclick={(e) => onDeleteClick({ altKey: e.altKey })}
    >
      Delete
    </button>
  {/if}
</div>
