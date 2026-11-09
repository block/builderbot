<!--
  ProjectTransferScanConfirm.svelte — the alert the transfer dialog raises
  when Transfer is clicked while the preflight scan is still reading the
  checkouts: files it hasn't listed yet may be lost on the remote, so the
  user confirms before the transfer starts at once.
-->
<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { SCAN_UNFINISHED_CONFIRMATION } from './projectTransfer';

  interface Props {
    open: boolean;
    /** Start the transfer without waiting for the scan. */
    onConfirm: () => void;
    /** Keep the dialog open and let the scan go on. */
    onCancel: () => void;
  }

  let { open, onConfirm, onCancel }: Props = $props();
</script>

<AlertDialog.Root {open} onOpenChange={(v) => !v && onCancel()}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Scan still running</AlertDialog.Title>
      <AlertDialog.Description>{SCAN_UNFINISHED_CONFIRMATION}</AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel onclick={onCancel}>Keep checking</AlertDialog.Cancel>
      <AlertDialog.Action variant="accent" onclick={onConfirm}>Transfer anyway</AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
