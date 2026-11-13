<!--
  Test fixture: a dialog shaped like the app's form dialogs (a field, a
  picker row, a suggestion list that can hold the focus, Cancel and a submit
  button, with a reply panel docked beside the form), or a viewer with no
  form, and a button on the page behind for focus to start on, for the
  Return key and open-focus tests of dialog-content.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import * as Dialog from '../index.js';

  let {
    open = $bindable(true),
    body = 'form',
    defaultAction,
    submitDisabled = false,
    focusField = false,
    onOpenAutoFocus,
    onSubmit,
    onCancel,
    onkeydown,
  }: {
    open?: boolean;
    /** A one-line form, one with a multiline editor, or nothing to submit. */
    body?: 'form' | 'textarea' | 'prompt' | 'viewer';
    defaultAction?: 'submit' | 'none';
    submitDisabled?: boolean;
    /** Focus the field as the dialog opens, the way the real dialogs do. */
    focusField?: boolean;
    onOpenAutoFocus?: (event: Event) => void;
    onSubmit?: () => void;
    onCancel?: () => void;
    onkeydown?: (event: KeyboardEvent) => void;
  } = $props();

  let field = $state<HTMLInputElement | null>(null);

  $effect(() => {
    if (open && focusField) void tick().then(() => field?.focus());
  });

  function handleSubmit(event: SubmitEvent) {
    event.preventDefault();
    onSubmit?.();
  }
</script>

<button type="button" data-testid="page-button">Elsewhere on the page</button>

<Dialog.Root bind:open>
  <Dialog.Content {defaultAction} {onOpenAutoFocus} {onkeydown}>
    <Dialog.Title>Move to Project</Dialog.Title>
    {#if body === 'viewer'}
      <p data-testid="text">Something to read.</p>
    {:else}
      <form onsubmit={handleSubmit}>
        <p data-testid="text">Pick a destination.</p>
        <input data-testid="field" bind:this={field} />
        {#if body === 'textarea'}
          <textarea data-testid="editor"></textarea>
        {:else if body === 'prompt'}
          <div data-testid="editor" contenteditable="true"></div>
        {/if}
        <button type="button" role="radio" aria-checked="false" data-testid="row">Alpha</button>
        <div role="listbox" tabindex="-1" aria-label="Suggestions" data-testid="listbox"></div>
        <button type="button" data-testid="cancel" onclick={onCancel}>Cancel</button>
        <button type="submit" data-testid="submit" disabled={submitDisabled}>Move</button>
      </form>
      <div role="dialog" aria-label="Reply" data-testid="docked-panel">
        <p tabindex="-1" data-testid="docked-text">Reply to the selection.</p>
      </div>
    {/if}
  </Dialog.Content>
</Dialog.Root>
