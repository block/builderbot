<!--
  Test stub for ConfirmDeleteDialog: renders the open request's copy and its
  confirm and cancel actions as plain elements, so component tests can drive
  a delete confirmation without the real alert dialog's portal and focus trap.
-->
<script lang="ts">
  import type { ConfirmDeleteRequest } from '../../../shared/ConfirmDeleteDialog.svelte';

  let {
    request,
    onCancel,
  }: {
    request: ConfirmDeleteRequest | null;
    onCancel: () => void;
  } = $props();
</script>

{#if request}
  <div data-testid="confirm-delete" data-title={request.title}>
    <p data-testid="confirm-delete-message">{request.message}</p>
    <button type="button" data-testid="confirm-delete-cancel" onclick={() => onCancel()}>
      Cancel
    </button>
    <button type="button" data-testid="confirm-delete-confirm" onclick={() => request.onConfirm()}>
      {request.confirmLabel ?? 'Delete'}
    </button>
  </div>
{/if}
