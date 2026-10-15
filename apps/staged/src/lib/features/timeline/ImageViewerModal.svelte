<!--
  ImageViewerModal.svelte — Full-size attachment viewer modal

  Loads and displays an image by its ID, or a PDF attachment in the webview's
  native PDF viewer (vector zoom, every page) by pointing a full-height frame
  at the attachment's served media URL, so the viewer streams the file with
  Range requests instead of the bytes crossing IPC as base64. Supports Escape
  to close and backdrop click dismissal.
-->
<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { getImageData } from '../../api/commands';
  import { viewport } from '../../shared/viewport.svelte';
  import ReferenceNavControls from '../references/ReferenceNavControls.svelte';
  import type { ReferenceNavState } from '../references/referenceHistory.svelte';
  import { resolvePdfAttachmentUrl } from '../../shared/markdown/mediaUrl';

  interface Props {
    open: boolean;
    imageId: string;
    filename: string;
    onClose: () => void;
    onDelete?: () => void;
    referenceNav?: ReferenceNavState;
  }

  let { open, imageId, filename, onClose, onDelete, referenceNav }: Props = $props();
  let dataUrl = $state<string | null>(null);
  let loading = $state(true);

  // Stored filenames take the extension of the sniffed format, so `.pdf`
  // means a PDF attachment: shown from its media URL, sized for a document.
  let isPdf = $derived(/\.pdf$/i.test(filename));
  let pdfUrl = $derived(isPdf ? resolvePdfAttachmentUrl(imageId) : null);

  $effect(() => {
    if (!open || isPdf) return;
    loading = true;
    dataUrl = null;
    let cancelled = false;
    getImageData(imageId)
      .then((url) => {
        if (cancelled) return;
        dataUrl = url;
        loading = false;
      })
      .catch(() => {
        if (!cancelled) loading = false;
      });
    return () => {
      cancelled = true;
    };
  });

  // Keys pressed inside the PDF frame do not reach the dialog. In Tauri the
  // frame is another origin (the staged-media scheme), so this listener
  // cannot attach and the close button and backdrop remain; in web mode the
  // frame is same-origin and Escape is forwarded where the viewer lets the
  // frame window see it.
  function forwardFrameEscape(e: Event) {
    const frame = e.currentTarget as HTMLIFrameElement;
    try {
      frame.contentWindow?.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') onClose();
      });
    } catch {
      // Cross-origin frame: the close button remains.
    }
  }
</script>

<Dialog.Root {open} onOpenChange={(v) => !v && onClose()}>
  <Dialog.Content
    class="max-w-[90vw] sm:max-w-[90vw] max-h-[90vh] bg-background p-0 gap-0 overflow-hidden border border-[var(--border-subtle)] flex flex-col {isPdf
      ? 'w-[90vw] h-[90vh]'
      : 'w-auto'}"
    showCloseButton={false}
  >
    <Dialog.Header
      class="flex-row items-center justify-between gap-3 px-4 py-3 border-b border-[var(--border-subtle)]"
    >
      {#if referenceNav}
        <ReferenceNavControls nav={referenceNav} />
      {/if}
      <Dialog.Title
        class="flex-1 min-w-0 text-[var(--size-sm)] font-medium text-foreground overflow-hidden text-ellipsis whitespace-nowrap"
        >{filename}</Dialog.Title
      >
      <div class="header-actions">
        {#if onDelete}
          <Button
            variant="ghost"
            size="icon-sm"
            title="Delete attachment"
            aria-label="Delete attachment"
            onclick={onDelete}
            class="size-7 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-destructive [&_svg]:!size-4"
          >
            <Trash2 size={16} />
          </Button>
        {/if}
        <Button
          variant="ghost"
          size="icon-sm"
          title={viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}
          aria-label="Close"
          onclick={onClose}
          class="size-7 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-[18px]"
        >
          <X size={18} />
        </Button>
      </div>
    </Dialog.Header>
    <div class="modal-body" class:pdf={isPdf}>
      {#if pdfUrl}
        <iframe class="pdf-frame" src={pdfUrl} title={filename} onload={forwardFrameEscape}
        ></iframe>
      {:else if loading}
        <div class="placeholder">Loading...</div>
      {:else if dataUrl}
        <img src={dataUrl} alt={filename} />
      {:else}
        <div class="placeholder error">Failed to load image</div>
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>

<style>
  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .modal-body {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 16px;
    overflow: auto;
    min-height: 200px;
  }

  .modal-body.pdf {
    flex: 1;
    min-height: 0;
    padding: 0;
  }

  .modal-body img {
    max-width: 100%;
    max-height: calc(90vh - 80px);
    object-fit: contain;
    border-radius: 4px;
  }

  .pdf-frame {
    flex: 1;
    width: 100%;
    height: 100%;
    border: 0;
    background: var(--bg-primary);
  }

  .placeholder {
    font-size: var(--size-sm);
    color: var(--text-muted);
    font-style: italic;
    padding: 40px;
  }

  .placeholder.error {
    color: var(--ui-danger);
  }
</style>
