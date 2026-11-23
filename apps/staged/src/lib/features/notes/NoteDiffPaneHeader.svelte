<!--
  NoteDiffPaneHeader.svelte — the row naming a version above its pane, while
  a note and its previous version show side by side.

  Both panes carry one, at one fixed height: the panes scroll together by
  keeping a line a third of the way down each level (noteDiffScroll.ts),
  which is only level on screen when the two scroll areas start at the same
  height.
-->
<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import { Button } from '$lib/components/ui/button';

  interface Props {
    label: string;
    /** Said after the label, such as when the version was saved. */
    detail?: string | null;
    /** Shows a close button, named by `closeLabel`. */
    onClose?: () => void;
    closeLabel?: string;
  }

  let { label, detail = null, onClose, closeLabel = 'Close' }: Props = $props();
</script>

<div class="note-diff-pane-header">
  <span class="pane-label">{label}</span>
  {#if detail}
    <span class="pane-detail">· {detail}</span>
  {/if}
  {#if onClose}
    <Button
      variant="ghost"
      size="icon-sm"
      class="ml-auto size-6 shrink-0 rounded-md text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-3.5"
      title={closeLabel}
      aria-label={closeLabel}
      onclick={onClose}
    >
      <X size={14} />
    </Button>
  {/if}
</div>

<style>
  .note-diff-pane-header {
    display: flex;
    flex-shrink: 0;
    align-items: center;
    gap: 6px;
    box-sizing: border-box;
    height: 32px;
    min-width: 0;
    padding: 0 12px 0 24px;
    border-bottom: 1px solid var(--border-subtle);
    background: var(--bg-chrome);
    color: var(--text-muted);
    font-size: var(--size-xs);
    white-space: nowrap;
  }

  .pane-label {
    color: var(--text-secondary);
    font-weight: 600;
  }

  .pane-detail {
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
