<!--
  Test fixture: a dialog with the app's toaster beside it (svelte-sonner's
  `<ol data-sonner-toaster>` stands in for the real Toaster), and a button
  on the page behind, for the outside-interaction tests of dialog-content.
-->
<script lang="ts">
  import * as Dialog from '../index.js';

  let {
    open = $bindable(true),
    onInteractOutside,
  }: {
    open?: boolean;
    onInteractOutside?: (event: PointerEvent) => void;
  } = $props();
</script>

<Dialog.Root bind:open>
  <Dialog.Content {onInteractOutside}>
    <Dialog.Title>A note</Dialog.Title>
    <p>Its content.</p>
  </Dialog.Content>
</Dialog.Root>

<ol data-sonner-toaster>
  <li data-sonner-toast>
    Quote removed from your reply
    <button type="button" data-testid="toast-undo">Undo</button>
  </li>
</ol>

<button type="button" data-testid="page-button">Elsewhere on the page</button>
