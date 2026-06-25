<script lang="ts">
  import Eye from '@lucide/svelte/icons/eye';
  import X from '@lucide/svelte/icons/x';
  import { Button } from '$lib/components/ui/button';

  interface ReferenceFile {
    path: string;
  }

  interface Props {
    referenceFiles: ReferenceFile[];
    selectedFile: string | null;
    onSelectFile: (path: string) => boolean | void | Promise<boolean | void>;
    onRemoveReferenceFile: (path: string) => void;
  }

  let { referenceFiles, selectedFile, onSelectFile, onRemoveReferenceFile }: Props = $props();
</script>

<div class="section-header">
  <div class="section-left"></div>
  <div class="section-divider">
    <span class="divider-label">REFERENCE</span>
    {#if referenceFiles.length > 0}
      <span class="count-capsule">{referenceFiles.length}</span>
    {/if}
  </div>
  <div class="section-right"></div>
</div>

{#if referenceFiles.length > 0}
  <ul class="tree-section reference-section">
    {#each referenceFiles as refFile (refFile.path)}
      <li class="tree-item-wrapper">
        <div
          class="tree-item file-item reference-item group/ref-item"
          class:selected={selectedFile === refFile.path}
          style="padding-left: 8px"
          title={refFile.path}
          role="button"
          tabindex="0"
          onclick={() => onSelectFile(refFile.path)}
          onkeydown={(e) => e.key === 'Enter' && onSelectFile(refFile.path)}
        >
          <span class="reference-icon"><Eye size={16} /></span>
          <span class="file-name truncate-start">{refFile.path}</span>
          <Button
            variant="ghost"
            size="icon"
            class="reference-remove-action ml-auto size-auto shrink-0 rounded-[3px] p-0.5 text-[var(--text-faint)] opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-hover)] hover:text-foreground group-hover/ref-item:opacity-100 [&_svg]:!size-3"
            title="Remove reference file"
            aria-label="Remove reference file"
            onclick={(e: MouseEvent) => {
              e.stopPropagation();
              onRemoveReferenceFile(refFile.path);
            }}
          >
            <X size={12} />
          </Button>
        </div>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .section-header {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    margin: 16px 12px 8px;
    gap: 6px;
  }

  .section-left {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    min-height: 1px;
  }

  .section-left::after {
    content: '';
    display: block;
    width: 100%;
    border-top: 1px solid var(--bg-hover);
  }

  .section-right {
    display: flex;
    align-items: center;
    justify-content: flex-start;
    min-height: 1px;
  }

  .section-right::before {
    content: '';
    display: block;
    width: 100%;
    border-top: 1px solid var(--bg-hover);
  }

  .section-divider {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
  }

  .divider-label {
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 700;
    letter-spacing: 0.05em;
    color: var(--text-faint);
    text-transform: uppercase;
  }

  .count-capsule {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: 10px;
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 700;
    background-color: var(--bg-hover);
    color: var(--text-faint);
  }

  .tree-section {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .tree-item-wrapper {
    margin: 0;
  }

  .tree-item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 6px;
    border: none;
    background: none;
    color: var(--text-muted);
    font-size: var(--size-sm);
    text-align: left;
    cursor: pointer;
    transition:
      background-color 0.08s,
      color 0.08s;
    min-height: 24px;
    border-radius: 0;
  }

  .tree-item:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .tree-item.selected {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .file-item {
    position: relative;
  }

  .file-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .truncate-start {
    direction: rtl;
    text-align: left;
    unicode-bidi: plaintext;
  }

  .reference-section {
    opacity: 0.85;
  }

  .reference-item {
    position: relative;
  }

  .reference-item:focus-within :global(.reference-remove-action),
  :global(.reference-remove-action:focus-visible) {
    opacity: 1;
  }

  @media (hover: none), (pointer: coarse) {
    :global(.reference-remove-action) {
      opacity: 1;
    }
  }

  .reference-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--text-muted);
  }
</style>
