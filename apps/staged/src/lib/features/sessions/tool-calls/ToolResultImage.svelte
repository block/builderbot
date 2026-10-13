<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog';
  import type { ToolCallImage } from '../toolCallImages';
  import { fetchToolImageSrc, immediateToolImageSrc } from '../toolImageSource';

  let { image }: { image: ToolCallImage } = $props();
  let container = $state<HTMLElement | null>(null);
  let dataUrl = $state<string | null>(null);
  let failed = $state(false);
  let open = $state(false);

  // Inline payloads and web `ref`s have a src straight away (the browser's
  // `loading="lazy"` defers the actual download). Anything that needs an IPC
  // round trip — refs under Tauri, file reads — waits until the image is
  // within a viewport of being seen, so expanding a card with twenty
  // screenshots doesn't decode all twenty at once.
  $effect(() => {
    let active = true;
    dataUrl = immediateToolImageSrc(image);
    failed = false;
    if (dataUrl !== null) return;

    const load = () => {
      fetchToolImageSrc(image).then(
        (url) => {
          if (active) dataUrl = url;
        },
        () => {
          if (active) failed = true;
        }
      );
    };
    const element = container;
    if (!element || typeof IntersectionObserver === 'undefined') {
      load();
      return () => {
        active = false;
      };
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          observer.disconnect();
          load();
        }
      },
      { rootMargin: '100%' }
    );
    observer.observe(element);
    return () => {
      active = false;
      observer.disconnect();
    };
  });
</script>

<div class="tool-result-image" bind:this={container}>
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
