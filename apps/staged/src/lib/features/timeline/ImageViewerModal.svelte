<!--
  ImageViewerModal.svelte — Full-size image viewer modal

  Loads and displays an image by its ID. Supports Escape to close and
  backdrop click dismissal.
-->
<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import * as Tooltip from '$lib/components/ui/tooltip';
  import { getImageData } from '../../api/commands';
  import { viewport } from '../../shared/viewport.svelte';

  interface Props {
    open: boolean;
    imageId: string;
    filename: string;
    onClose: () => void;
    onDelete?: () => void;
  }

  let { open, imageId, filename, onClose, onDelete }: Props = $props();
  let dataUrl = $state<string | null>(null);
  let loading = $state(true);

  $effect(() => {
    if (!open) return;
    loading = true;
    dataUrl = null;
    getImageData(imageId)
      .then((url) => {
        dataUrl = url;
        loading = false;
      })
      .catch(() => {
        loading = false;
      });
  });
</script>

<Dialog.Root {open} onOpenChange={(v) => !v && onClose()}>
  <Dialog.Content
    class="max-w-[90vw] max-h-[90vh] w-auto bg-background p-0 gap-0 overflow-hidden border border-[var(--border-subtle)] flex flex-col"
    showCloseButton={false}
  >
    <Dialog.Header
      class="flex-row items-center justify-between gap-3 px-4 py-3 border-b border-[var(--border-subtle)]"
    >
      <Dialog.Title
        class="flex-1 min-w-0 text-[var(--size-sm)] font-medium text-foreground overflow-hidden text-ellipsis whitespace-nowrap"
        >{filename}</Dialog.Title
      >
      <div class="header-actions">
        {#if onDelete}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="ghost"
                  size="icon-sm"
                  onclick={onDelete}
                  class="size-7 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-destructive [&_svg]:!size-4"
                >
                  <Trash2 size={16} />
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Delete image</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        <Tooltip.Root>
          <Tooltip.Trigger>
            {#snippet child({ props })}
              <Button
                {...props}
                variant="ghost"
                size="icon-sm"
                onclick={onClose}
                class="size-7 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-[18px]"
              >
                <X size={18} />
              </Button>
            {/snippet}
          </Tooltip.Trigger>
          <Tooltip.Content>
            {viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}
          </Tooltip.Content>
        </Tooltip.Root>
      </div>
    </Dialog.Header>
    <div class="modal-body">
      {#if loading}
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

  .modal-body img {
    max-width: 100%;
    max-height: calc(90vh - 80px);
    object-fit: contain;
    border-radius: 4px;
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
