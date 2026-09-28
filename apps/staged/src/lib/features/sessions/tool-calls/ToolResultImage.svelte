<script lang="ts">
  import { readImageFile } from '$lib/api/commands';
  import * as Dialog from '$lib/components/ui/dialog';
  import type { ToolCallImage } from '../toolCallImages';

  let { image }: { image: ToolCallImage } = $props();
  let dataUrl = $state<string | null>(null);
  let failed = $state(false);
  let open = $state(false);

  $effect(() => {
    let active = true;
    dataUrl = null;
    failed = false;
    if (image.kind === 'inline') {
      dataUrl = image.dataUrl;
    } else {
      readImageFile(image.path).then(
        (url) => {
          if (active) dataUrl = url;
        },
        () => {
          if (active) failed = true;
        }
      );
    }
    return () => {
      active = false;
    };
  });
</script>

<div class="tool-result-image">
  {#if failed}
    <div class="image-placeholder" title={image.label}>Image unavailable</div>
  {:else if dataUrl}
    <button type="button" class="image-preview" title="Expand image" onclick={() => (open = true)}>
      <img src={dataUrl} alt={image.label} loading="lazy" onerror={() => (failed = true)} />
    </button>
  {:else}
    <div class="image-placeholder" role="status">Loading image...</div>
  {/if}
</div>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-[90vw] sm:max-w-[90vw] max-h-[90vh] w-auto overflow-auto">
    <Dialog.Header>
      <Dialog.Title>Tool result image</Dialog.Title>
    </Dialog.Header>
    {#if dataUrl}
      <img class="full-image" src={dataUrl} alt={image.label} />
    {/if}
  </Dialog.Content>
</Dialog.Root>

<style>
  .tool-result-image {
    min-width: 0;
    margin-top: 8px;
  }

  img {
    display: block;
    max-width: 100%;
    max-height: 300px;
    object-fit: contain;
    border-radius: 4px;
  }

  .image-preview {
    display: block;
    max-width: 100%;
    padding: 0;
    border: 0;
    background: transparent;
    cursor: zoom-in;
  }

  .image-preview:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: 2px;
  }

  .full-image {
    max-height: calc(90vh - 100px);
  }

  .image-placeholder {
    padding: 16px;
    color: var(--text-muted);
    background: var(--bg-primary);
    border-radius: 4px;
  }
</style>
