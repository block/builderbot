<!--
  ConfirmDialog.svelte - Themed confirmation dialog

  A modal dialog for confirming destructive actions, styled to match the app theme.

  Usage:
    <ConfirmDialog
      title="Delete Branch"
      message="Are you sure?"
      confirmLabel="Delete"
      danger={true}
      onConfirm={() => doDelete()}
      onCancel={() => closeDialog()}
    />
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { AlertTriangle } from 'lucide-svelte';
  import { createBackdropDismissHandlers } from './backdropDismiss';

  interface Props {
    title?: string;
    message: string;
    confirmLabel?: string;
    cancelLabel?: string;
    danger?: boolean;
    confirmDisabled?: boolean;
    cancelDisabled?: boolean;
    error?: string | null;
    onConfirm: () => void;
    onCancel: () => void;
  }

  let {
    title = 'Confirm',
    message,
    confirmLabel = 'Confirm',
    cancelLabel = 'Cancel',
    danger = false,
    confirmDisabled = false,
    cancelDisabled = false,
    error = null,
    onConfirm,
    onCancel,
  }: Props = $props();
  const backdropDismiss = createBackdropDismissHandlers({
    onDismiss: () => !cancelDisabled && onCancel(),
  });

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && !cancelDisabled) {
      onCancel();
      event.preventDefault();
    } else if (event.key === 'Enter' && !confirmDisabled) {
      onConfirm();
      event.preventDefault();
    }
  }

  // Portal the dialog to document.body so it escapes any intermediate
  // stacking contexts (e.g. sticky headers inside scroll containers).
  let backdropEl: HTMLDivElement;
  onMount(() => {
    document.body.appendChild(backdropEl);
    return () => {
      backdropEl?.remove();
    };
  });
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  bind:this={backdropEl}
  onpointerdown={backdropDismiss.handlePointerDown}
  onclick={backdropDismiss.handleClick}
  onkeydown={(e) => e.key === 'Escape' && !cancelDisabled && onCancel()}
>
  <div class="modal" class:danger>
    <div class="modal-content">
      {#if danger}
        <div class="icon-wrapper">
          <AlertTriangle size={24} />
        </div>
      {/if}
      <div class="text-content">
        <h2>{title}</h2>
        <p>{message}</p>
        {#if error}
          <p class="error-text">{error}</p>
        {/if}
      </div>
    </div>

    <div class="modal-actions">
      <button class="btn btn-secondary" onclick={onCancel} disabled={cancelDisabled}>
        {cancelLabel}
      </button>
      <button
        class="btn"
        class:btn-danger={danger}
        class:btn-primary={!danger}
        onclick={onConfirm}
        disabled={confirmDisabled}
      >
        {confirmLabel}
      </button>
    </div>
  </div>
</div>

<style>
  @keyframes fadeIn {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  @keyframes scaleIn {
    from {
      opacity: 0;
      transform: scale(0.96);
    }
    to {
      opacity: 1;
      transform: scale(1);
    }
  }

  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    animation: fadeIn 0.15s ease;
  }

  .modal {
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 400px;
    max-width: 90vw;
    overflow: hidden;
    animation: scaleIn 0.15s ease;
  }

  .modal-content {
    display: flex;
    gap: 16px;
    padding: 24px;
  }

  .icon-wrapper {
    flex-shrink: 0;
    width: 40px;
    height: 40px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--ui-danger-bg);
    border-radius: 10px;
    color: var(--ui-danger);
  }

  .text-content {
    flex: 1;
    min-width: 0;
  }

  .text-content h2 {
    margin: 0 0 8px 0;
    font-size: var(--size-base);
    font-weight: 600;
    color: var(--text-primary);
  }

  .text-content p {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
    line-height: 1.5;
  }

  .text-content .error-text {
    margin-top: 8px;
    color: var(--color-danger, #f85149);
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 16px 24px;
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-primary);
    /* Prevent layout shift when button labels change width */
    align-items: center;
  }

  .btn {
    padding: 8px 16px;
    min-width: 80px;
    border: none;
    border-radius: 6px;
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      opacity 0.15s ease;
  }

  .btn-secondary {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-secondary:hover:not(:disabled) {
    background: var(--border-subtle);
  }

  .btn-primary {
    background: var(--ui-accent);
    color: var(--bg-primary);
  }

  .btn-primary:hover:not(:disabled) {
    background: var(--ui-accent-hover);
  }

  .btn-danger {
    background: var(--ui-danger);
    color: white;
  }

  .btn-danger:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
