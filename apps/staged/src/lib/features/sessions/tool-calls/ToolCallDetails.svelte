<script lang="ts">
  import type { RichToolItem } from '../acpTranscript';
  import type { DisplayRootInput } from '../pathDisplayRoots';
  import type { ToolCallViewModel } from '../toolCallViewModel';
  import CommandToolDetails from './CommandToolDetails.svelte';
  import EditToolDetails from './EditToolDetails.svelte';
  import GenericToolDetails from './GenericToolDetails.svelte';
  import NetworkToolDetails from './NetworkToolDetails.svelte';
  import ReadSearchToolDetails from './ReadSearchToolDetails.svelte';

  interface Props {
    item: RichToolItem;
    viewModel: ToolCallViewModel;
    displayRoots?: DisplayRootInput;
  }

  let { item, viewModel, displayRoots }: Props = $props();
</script>

<div class="tool-code-block">
  {#if viewModel.category === 'edit'}
    <EditToolDetails {viewModel} />
  {:else if viewModel.category === 'command'}
    <CommandToolDetails {viewModel} />
  {:else if viewModel.category === 'read' || viewModel.category === 'search'}
    <ReadSearchToolDetails {item} {viewModel} {displayRoots} />
  {:else if viewModel.category === 'network'}
    <NetworkToolDetails {item} {viewModel} />
  {:else}
    <GenericToolDetails {viewModel} />
  {/if}
</div>

<style>
  .tool-code-block {
    margin-top: 4px;
    border-radius: 8px;
    padding: 12px 14px;
    max-height: min(360px, 52vh);
    overflow-y: auto;
    background: color-mix(in srgb, var(--bg-chrome) 82%, var(--bg-primary));
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) * 0.9);
    line-height: 1.5;
  }

  .tool-code-block::-webkit-scrollbar,
  .tool-code-block :global(.tool-code-output)::-webkit-scrollbar {
    width: 4px;
    height: 4px;
  }

  .tool-code-block::-webkit-scrollbar-track,
  .tool-code-block :global(.tool-code-output)::-webkit-scrollbar-track {
    background: transparent;
  }

  .tool-code-block::-webkit-scrollbar-thumb,
  .tool-code-block :global(.tool-code-output)::-webkit-scrollbar-thumb {
    border-radius: 2px;
    background: var(--scrollbar-thumb-transparent);
  }

  .tool-code-block :global(.tool-detail-stack) {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  .tool-code-block :global(.tool-primary-row),
  .tool-code-block :global(.tool-meta-row) {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    align-items: center;
    min-width: 0;
  }

  .tool-code-block :global(.tool-field-list) {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 4px 8px;
    min-width: 0;
  }

  .tool-code-block :global(.tool-field-label) {
    color: var(--text-faint);
    font-weight: 600;
  }

  .tool-code-block :global(.tool-field-value) {
    min-width: 0;
    overflow-wrap: anywhere;
    color: var(--text-muted);
  }

  .tool-code-block :global(.tool-command-panel) {
    display: flex;
    flex-direction: column;
    gap: 6px;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    padding: 8px 10px;
    background: var(--bg-primary);
  }

  .tool-code-block :global(.tool-network-line) {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: baseline;
    min-width: 0;
    color: var(--text-primary);
    font-weight: 500;
  }

  /* Lay the command out like a terminal line: the "$" sits in a fixed-width
     gutter and the command flows beside it, wrapping under itself with a
     hanging indent instead of stranding the prompt on its own row. */
  .tool-code-block :global(.tool-command-line) {
    min-width: 0;
    padding-left: 1.25em;
    color: var(--text-primary);
    font-weight: 500;
  }

  .tool-code-block :global(.tool-command-prefix) {
    display: inline-block;
    width: 1.25em;
    margin-left: -1.25em;
    color: var(--text-faint);
    user-select: none;
  }

  .tool-code-block :global(.tool-command-text) {
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .tool-code-block :global(.tool-network-url) {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .tool-code-block :global(.tool-status-badge),
  .tool-code-block :global(.tool-chip) {
    max-width: 100%;
    overflow: hidden;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    padding: 2px 6px;
    color: var(--text-muted);
    font-family: inherit;
    font-size: calc(var(--size-xs) * 0.88);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tool-code-block :global(.tool-status-badge.success) {
    border-color: color-mix(in srgb, var(--ui-success, var(--ui-accent)) 35%, var(--border-subtle));
    color: var(--ui-success, var(--ui-accent));
  }

  .tool-code-block :global(.tool-status-badge.danger) {
    border-color: color-mix(in srgb, var(--ui-danger) 35%, var(--border-subtle));
    color: var(--ui-danger);
  }

  .tool-code-block :global(.tool-panel-label) {
    margin: 10px 0 4px;
    color: var(--text-faint);
    font-family: inherit;
    font-size: calc(var(--size-xs) * 0.86);
    font-weight: 600;
    letter-spacing: 0;
  }

  .tool-code-block :global(.tool-panel-label:first-child) {
    margin-top: 0;
  }

  .tool-code-block :global(.tool-output-section) {
    min-width: 0;
  }

  .tool-code-block :global(.tool-output-body) {
    position: relative;
    min-width: 0;
  }

  .tool-code-block :global(.tool-output-body-actions) {
    position: absolute;
    top: 4px;
    right: 4px;
    z-index: 1;
  }

  .tool-code-block :global(.tool-output-header) {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-top: 10px;
  }

  .tool-code-block :global(.tool-output-header .tool-panel-label) {
    margin: 0 0 4px;
  }

  .tool-code-block :global(.tool-copy-button) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    flex-shrink: 0;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--text-faint);
    cursor: pointer;
  }

  .tool-code-block :global(.tool-copy-button:hover),
  .tool-code-block :global(.tool-copy-button:focus-visible) {
    background: var(--bg-hover);
    color: var(--text-muted);
    outline: none;
  }

  /* The overlaid button sits over the top-right of the output, so give it the
     block's own background to mask any text it covers. */
  .tool-code-block :global(.tool-output-body-actions .tool-copy-button) {
    background: color-mix(in srgb, var(--bg-chrome) 82%, var(--bg-primary));
  }

  .tool-code-block :global(.tool-output-body-actions .tool-copy-button:hover),
  .tool-code-block :global(.tool-output-body-actions .tool-copy-button:focus-visible) {
    background: var(--bg-hover);
  }

  .tool-code-block :global(.tool-code-output) {
    margin: 0;
    max-height: 220px;
    overflow: auto;
    color: var(--text-muted);
    white-space: pre-wrap;
    word-break: break-word;
  }

  .tool-code-block :global(.tool-code-output.tool-output-danger) {
    color: var(--ui-danger);
  }

  .tool-code-block :global(.tool-code-output.tool-output-cancelled) {
    color: var(--text-faint);
  }

  .tool-code-block :global(.tool-empty-row) {
    border: 1px dashed var(--border-subtle);
    border-radius: 8px;
    padding: 8px 10px;
    color: var(--text-faint);
  }

  .tool-code-block :global(.tool-match-list) {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .tool-code-block :global(.tool-match-row) {
    display: grid;
    grid-template-columns: minmax(120px, max-content) minmax(0, 1fr);
    gap: 8px;
    align-items: baseline;
    min-width: 0;
  }

  .tool-code-block :global(.tool-match-snippet) {
    min-width: 0;
    overflow-wrap: anywhere;
    color: var(--text-muted);
    white-space: pre-wrap;
  }

  .tool-code-block :global(.tool-code-status) {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 3px;
    margin-top: 8px;
    color: var(--text-muted);
    font-size: calc(var(--size-xs) * 0.85);
  }

  .tool-code-block :global(.tool-code-status.status-danger) {
    color: var(--ui-danger);
  }

  .tool-code-block :global(.tool-code-status.status-cancelled) {
    color: var(--text-faint);
  }
</style>
