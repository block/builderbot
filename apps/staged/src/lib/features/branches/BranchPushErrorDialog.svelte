<!--
  BranchPushErrorDialog.svelte — the alert the PR button raises when a push
  fails: the error, Cancel (which clears the failed push) and Retry (which
  repeats it as it was asked for).

  A plain Return presses Retry, except when the retry would force push.
  Rewriting the remote branch is confirmed with a click: the force-push
  confirmation keeps Return inert, and its retry does the same, since this
  dialog can be reopened from the PR button long after the push failed, when
  the remote may have moved again.
-->
<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';

  interface Props {
    /** Closing with Escape only hides the dialog; the failed push stays to reopen it. */
    open: boolean;
    /** Whether the failed push, and so the retry, is a force push. */
    force: boolean;
    error: string | null;
    onRetry: () => void;
    onCancel: () => void;
  }

  let { open = $bindable(), force, error, onRetry, onCancel }: Props = $props();
</script>

<AlertDialog.Root bind:open>
  <AlertDialog.Content defaultAction={force ? 'none' : undefined}>
    <AlertDialog.Header>
      <AlertDialog.Title>{force ? 'Force Push Failed' : 'Push Failed'}</AlertDialog.Title>
      <AlertDialog.Description class="max-h-[42vh] overflow-auto whitespace-pre-line">
        {error ?? 'An unknown error occurred while pushing.'}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel onclick={onCancel}>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action variant="outline" onclick={onRetry}>Retry</AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
