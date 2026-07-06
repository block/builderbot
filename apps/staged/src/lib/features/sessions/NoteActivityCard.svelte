<script lang="ts">
  import FileText from '@lucide/svelte/icons/file-text';

  interface Props {
    label: string;
    onclick?: () => void;
  }

  let { label, onclick }: Props = $props();
</script>

{#if onclick}
  <button type="button" class="note-activity-card clickable" {onclick}>
    <FileText size={14} aria-hidden="true" />
    <span>{label}</span>
  </button>
{:else}
  <div class="note-activity-card">
    <FileText size={14} aria-hidden="true" />
    <span>{label}</span>
  </div>
{/if}

<style>
  .note-activity-card {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    max-width: 100%;
    margin-top: 8px;
    padding: 8px 10px;
    border: 1px solid transparent;
    border-radius: 8px;
    background: var(--note-bg);
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

  button.note-activity-card {
    cursor: pointer;
  }

  button.note-activity-card:hover {
    background: var(--note-bg-emphasis);
  }

  button.note-activity-card:focus-visible {
    outline: 2px solid var(--note-color);
    outline-offset: 2px;
  }
</style>
