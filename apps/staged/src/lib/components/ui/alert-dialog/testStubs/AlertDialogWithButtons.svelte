<!--
  Test fixture: an alert dialog with a Cancel and (optionally) an Action,
  and a button on the page behind for focus to start on, for the Return key
  tests of alert-dialog-content.
-->
<script lang="ts">
  import * as AlertDialog from '../index.js';
  import type { ButtonVariant } from '$lib/components/ui/button/index.js';

  let {
    open = $bindable(true),
    variant,
    defaultAction,
    withAction = true,
    actionDisabled = false,
    cancelDisabled = false,
    onConfirm,
    onCancel,
    onOpenChange,
    onkeydown,
  }: {
    open?: boolean;
    variant?: ButtonVariant;
    defaultAction?: 'action' | 'cancel' | 'none';
    withAction?: boolean;
    actionDisabled?: boolean;
    cancelDisabled?: boolean;
    onConfirm?: () => void;
    onCancel?: () => void;
    onOpenChange?: (open: boolean) => void;
    onkeydown?: (event: KeyboardEvent) => void;
  } = $props();
</script>

<button type="button" data-testid="page-button">Elsewhere on the page</button>

<AlertDialog.Root bind:open {onOpenChange}>
  <AlertDialog.Content {defaultAction} {onkeydown}>
    <AlertDialog.Header>
      <AlertDialog.Title>Scan still running</AlertDialog.Title>
      <AlertDialog.Description data-testid="description">
        Files not listed yet may be lost.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={cancelDisabled} onclick={onCancel}>Cancel</AlertDialog.Cancel>
      {#if withAction}
        <AlertDialog.Action {variant} disabled={actionDisabled} onclick={onConfirm}>
          Confirm
        </AlertDialog.Action>
      {/if}
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
