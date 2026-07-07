<script lang="ts">
  import FileText from '@lucide/svelte/icons/file-text';

  interface Props {
    label: string;
  }

  let { label }: Props = $props();
  let liveMode: 'polite' | undefined = $derived(label === 'Writing note' ? 'polite' : undefined);
</script>

<div class="note-activity-card" aria-live={liveMode}>
  <FileText size={14} aria-hidden="true" />
  <span>{label}</span>
</div>

<style>
  .note-activity-card {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    max-width: 100%;
    margin-top: 8px;
    padding: 7px 10px;
    border: 1px solid color-mix(in srgb, var(--note-color) 45%, transparent);
    border-radius: 8px;
    background: transparent;
    color: var(--note-color);
    font: inherit;
    font-size: var(--size-xs);
    font-weight: 600;
    line-height: 1.2;
    text-align: left;
  }

  .note-activity-card :global(svg) {
    flex-shrink: 0;
  }

  .note-activity-card span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
