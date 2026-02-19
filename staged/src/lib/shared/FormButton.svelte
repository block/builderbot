<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    variant?: 'default' | 'primary' | 'ghost';
    disabled?: boolean;
    onclick?: (e: MouseEvent) => void;
    class?: string;
    children: Snippet;
  }

  let {
    variant = 'default',
    disabled = false,
    onclick,
    class: className = '',
    children,
  }: Props = $props();
</script>

<button
  class="form-btn {className}"
  class:primary={variant === 'primary'}
  class:ghost={variant === 'ghost'}
  {disabled}
  {onclick}
>
  {@render children()}
</button>

<style>
  .form-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    min-height: 36px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: transparent;
    color: var(--text-muted);
    padding: 8px 12px;
    font-size: var(--size-sm);
    font-family: inherit;
    cursor: pointer;
    transition:
      border-color 0.15s ease,
      color 0.15s ease,
      background-color 0.15s ease;
  }

  .form-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
    background: var(--bg-hover);
  }

  .form-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .form-btn.primary {
    background: var(--ui-accent);
    border-color: var(--ui-accent);
    color: var(--bg-deepest);
    font-weight: 600;
  }

  .form-btn.primary:hover:not(:disabled) {
    background: var(--ui-accent-hover);
    border-color: var(--ui-accent-hover);
  }

  .form-btn.ghost {
    border-color: transparent;
    color: var(--text-muted);
  }

  .form-btn.ghost:hover {
    border-color: transparent;
    background: var(--bg-hover);
    color: var(--text-primary);
  }
</style>
