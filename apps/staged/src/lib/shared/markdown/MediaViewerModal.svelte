<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import type { MediaKind } from './mediaUrl';

  interface Props {
    open: boolean;
    src: string;
    kind: MediaKind;
    title: string;
    onClose: () => void;
  }

  let { open, src, kind, title, onClose }: Props = $props();
  let failed = $state(false);
  $effect(() => {
    if (open && src) failed = false;
  });
</script>

<Dialog.Root {open} onOpenChange={(value) => !value && onClose()}>
  <Dialog.Content
    class="max-w-[90vw] sm:max-w-[90vw] max-h-[90vh] w-auto bg-background p-0 gap-0 overflow-hidden border border-[var(--border-subtle)] flex flex-col"
    showCloseButton={false}
  >
    <Dialog.Header
      class="flex-row items-center justify-between gap-3 px-4 py-3 border-b border-[var(--border-subtle)]"
    >
      <Dialog.Title class="min-w-0 text-[var(--size-sm)] font-medium text-foreground truncate"
        >{title || (kind === 'video' ? 'Video' : 'Image')}</Dialog.Title
      >
      <Button variant="ghost" size="sm" onclick={onClose}>Close</Button>
    </Dialog.Header>
    <div class="media-viewer-body">
      {#if open}
        {#key src}
          {#if kind === 'video'}
            <!-- Agent recordings need not contain speech; no caption track is available. -->
            <!-- svelte-ignore a11y_media_has_caption -->
            <video
              {src}
              controls
              autoplay
              playsinline
              aria-label={title || 'Video'}
              onerror={() => (failed = true)}
            ></video>
          {:else}
            <img {src} alt={title} onerror={() => (failed = true)} />
          {/if}
        {/key}
        {#if failed}<p role="status">
            Unable to display this media. The file may be missing or its codec unsupported.
          </p>{/if}
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>

<style>
  .media-viewer-body {
    min-height: 120px;
    padding: 16px;
    overflow: auto;
  }
  .media-viewer-body img,
  .media-viewer-body video {
    display: block;
    max-width: 100%;
    max-height: calc(90vh - 100px);
    margin: auto;
    object-fit: contain;
    border-radius: 4px;
  }
  .media-viewer-body p {
    color: var(--text-muted);
    font-size: var(--size-sm);
  }
</style>
