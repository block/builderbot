<script lang="ts">
  import type { HTMLInputAttributes } from 'svelte/elements';

  interface Props extends Omit<HTMLInputAttributes, 'value'> {
    value?: string;
    autofocus?: boolean;
  }

  let { value = $bindable(''), autofocus = false, ...rest }: Props = $props();

  function autoFocusAction(node: HTMLElement) {
    if (autofocus) node.focus();
  }
</script>

<input class="form-input" bind:value use:autoFocusAction {...rest} />

<style>
  .form-input {
    min-height: 42px;
    border: 1.5px solid var(--border-muted);
    background: transparent;
    color: var(--text-primary);
    border-radius: 10px;
    padding: 10px 14px;
    font-size: var(--size-md);
    font-family: inherit;
    outline: none;
    transition: border-color 0.15s ease;
  }

  .form-input:focus {
    border-color: var(--ui-accent);
  }

  .form-input::placeholder {
    color: var(--text-faint);
  }

  .form-input:disabled {
    opacity: 0.6;
  }
</style>
