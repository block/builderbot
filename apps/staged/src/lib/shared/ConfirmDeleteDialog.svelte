<!--
  ConfirmDeleteDialog.svelte - Confirmation for deleting or discarding an item

  Driven by a single request: set one to open the dialog, and clear it from
  `onCancel` (and from the request's own `onConfirm`) to close it. The branch
  card and the project notes list share it, so deleting a row asks the same
  way in both.
-->
<script module lang="ts">
  export type ConfirmDeleteRequest = {
    title: string;
    message: string;
    /** Label for the destructive button; defaults to "Delete". */
    confirmLabel?: string;
    onConfirm: () => void;
  };
</script>

<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';

  interface Props {
    request: ConfirmDeleteRequest | null;
    onCancel: () => void;
  }

  let { request, onCancel }: Props = $props();
</script>

{#if request}
  <AlertDialog.Root open={true} onOpenChange={(v) => !v && onCancel()}>
    <AlertDialog.Content>
      <AlertDialog.Header>
        <AlertDialog.Title>{request.title}</AlertDialog.Title>
        <AlertDialog.Description>{request.message}</AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action variant="destructive" onclick={request.onConfirm}>
          {request.confirmLabel ?? 'Delete'}
        </AlertDialog.Action>
      </AlertDialog.Footer>
    </AlertDialog.Content>
  </AlertDialog.Root>
{/if}
