<!--
  ImageViewerModal.svelte — Full-size image viewer modal

  Loads and displays an image by its ID. Supports Escape to close and
  backdrop click dismissal.
-->
<script lang="ts">
  import { X, Trash2 } from 'lucide-svelte';
  import { getImageData } from '../../api/commands';
  import { createBackdropDismissHandlers } from '../../shared/backdropDismiss';

  interface Props {
    imageId: string;
    filename: string;
    onClose: () => void;
    onDelete?: () => void;
  }

  let { imageId, filename, onClose, onDelete }: Props = $props();
  let dataUrl = $state<string | null>(null);
  let loading = $state(true);
  const backdropDismiss = createBackdropDismissHandlers({ onDismiss: () => onClose() });

  $effect(() => {
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

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onpointerdown={backdropDismiss.handlePointerDown}
  onclick={backdropDismiss.handleClick}
>
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div class="modal" role="presentation" onclick={(e) => e.stopPropagation()}>
    <header class="modal-header">
      <span class="filename">{filename}</span>
      <div class="header-actions">
        {#if onDelete}
          <button class="delete-btn" onclick={onDelete} title="Delete image">
            <Trash2 size={16} />
          </button>
        {/if}
        <button class="close-btn" onclick={onClose} title="Close (Esc)">
          <X size={18} />
        </button>
      </div>
    </header>
    <div class="modal-body">
      {#if loading}
        <div class="placeholder">Loading...</div>
      {:else if dataUrl}
        <img src={dataUrl} alt={filename} />
      {:else}
        <div class="placeholder error">Failed to load image</div>
      {/if}
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    display: flex;
    flex-direction: column;
    max-width: 90vw;
    max-height: 90vh;
    background: var(--bg-primary);
    border-radius: 12px;
    border: 1px solid var(--border-subtle);
    box-shadow: var(--shadow-elevated);
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-subtle);
    gap: 12px;
  }

  .filename {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    flex-shrink: 0;
    transition:
      color 0.15s,
      background-color 0.15s;
  }

  .close-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .delete-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    flex-shrink: 0;
    transition:
      color 0.15s,
      background-color 0.15s;
  }

  .delete-btn:hover {
    color: var(--ui-danger);
    background-color: var(--bg-hover);
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

  @media (max-width: 640px) {
    .modal {
      width: 100vw;
      max-width: none;
      height: 100vh;
      height: 100dvh;
      max-height: none;
      border-radius: 0;
      box-shadow: none;
    }

    .close-btn,
    .delete-btn {
      width: 40px;
      height: 40px;
    }

    .modal-body {
      padding: 12px;
    }

    .modal-body img {
      max-height: calc(100dvh - 80px);
    }
  }
</style>
