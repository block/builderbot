<!--
  BranchCardFooter.svelte - Shared footer component for branch cards

  Displays "New note" and "New commit" buttons with consistent styling.
  Used by both BranchCard and RemoteBranchCard.
-->
<script lang="ts">
  import { GitCommitHorizontal, FileText } from 'lucide-svelte';
  import type { BranchSessionType } from './types';

  interface Props {
    disabled?: boolean;
    onNewSession: (mode: BranchSessionType) => void;
  }

  let { disabled = false, onNewSession }: Props = $props();
</script>

<div class="new-btn-group">
  <button
    class="new-item-btn note-btn"
    onclick={() => onNewSession('note')}
    {disabled}
    title="New note"
  >
    <FileText size={13} />
    <span>New note</span>
  </button>
  <button
    class="new-item-btn commit-btn"
    onclick={() => onNewSession('commit')}
    {disabled}
    title="New commit"
  >
    <GitCommitHorizontal size={13} />
    <span>New commit</span>
  </button>
</div>

<style>
  /* Footer button group for note/commit buttons */
  .new-btn-group {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .new-item-btn {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 4px 10px;
    background: none;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    color: var(--text-faint);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    transition:
      color 0.15s,
      border-color 0.15s,
      background-color 0.15s;
    white-space: nowrap;
  }

  .new-item-btn:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--border-muted);
    background: var(--bg-hover);
  }

  .new-item-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .new-item-btn :global(svg) {
    flex-shrink: 0;
    transition: color 0.15s;
  }

  /* Icon color on button hover */
  .note-btn:hover :global(svg) {
    color: var(--note-color);
  }

  .commit-btn:hover :global(svg) {
    color: var(--commit-color);
  }
</style>
