<!--
  ConfirmDialog.svelte - Themed confirmation dialog

  A modal dialog for confirming destructive actions, styled to match the app theme.
  Replaces the native system dialog for a consistent look.

  Usage:
    <ConfirmDialog
      title="Delete Branch"
      message="Are you sure you want to delete this branch?"
      confirmLabel="Delete"
      danger={true}
      onConfirm={() => doDelete()}
      onCancel={() => closeDialog()}
    />
-->
<script lang="ts">
  import { AlertTriangle } from 'lucide-svelte';

  interface Props {
    title?: string;
    message: string;
    confirmLabel?: string;
    cancelLabel?: string;
    danger?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
  }

  let {
    title = 'Confirm',
    message,
    confirmLabel = 'Confirm',
    cancelLabel = 'Cancel',
    danger = false,
    onConfirm,
    onCancel,
  }: Props = $props();

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      onCancel();
      event.preventDefault();
    } else if (event.key === 'Enter') {
      onConfirm();
      event.preventDefault();
    }
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      onCancel();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={(e) => e.key === 'Escape' && onCancel()}
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
      </div>
    </div>

    <div class="modal-actions">
      <button class="btn btn-secondary" onclick={onCancel}>
        {cancelLabel}
      </button>
      <button class="btn" class:btn-danger={danger} class:btn-primary={!danger} onclick={onConfirm}>
        {confirmLabel}
      </button>
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
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 400px;
    max-width: 90vw;
    overflow: hidden;
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

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 16px 24px;
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-primary);
  }

  .btn {
    padding: 8px 16px;
    border: none;
    border-radius: 6px;
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition:
      background-color 0.1s,
      opacity 0.1s;
  }

  .btn-secondary {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-secondary:hover {
    background: var(--border-subtle);
  }

  .btn-primary {
    background: var(--ui-accent);
    color: var(--bg-primary);
  }

  .btn-primary:hover {
    background: var(--ui-accent-hover);
  }

  .btn-danger {
    background: var(--ui-danger);
    color: white;
  }

  .btn-danger:hover {
    filter: brightness(1.1);
  }
</style>
