<script lang="ts" generics="T extends string">
  interface Props {
    options: { value: T; label: string }[];
    value: T;
    disabled?: boolean;
  }

  let { options, value = $bindable(), disabled = false }: Props = $props();
</script>

<div class="toggle-group">
  {#each options as option}
    <button
      class="toggle-card"
      class:active={value === option.value}
      {disabled}
      onclick={() => (value = option.value)}
    >
      <span class="radio-indicator"></span>
      <span class="toggle-label">{option.label}</span>
    </button>
  {/each}
</div>

<style>
  .toggle-group {
    display: flex;
    gap: 8px;
  }

  .toggle-card {
    flex: 1;
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 56px;
    border: 1.5px solid var(--border-muted);
    border-radius: 10px;
    background: transparent;
    color: var(--text-muted);
    padding: 14px 12px;
    font-family: inherit;
    cursor: pointer;
    transition:
      border-color 0.2s ease,
      color 0.2s ease,
      background-color 0.2s ease;
  }

  .toggle-card:hover {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
  }

  .toggle-card.active {
    border-color: var(--ui-accent);
    color: var(--text-primary);
  }

  .toggle-card:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .radio-indicator {
    position: absolute;
    top: 8px;
    right: 8px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 1.5px solid var(--border-muted);
    background: transparent;
    transition:
      border-color 0.2s ease,
      background-color 0.2s ease,
      box-shadow 0.2s ease;
  }

  .toggle-card.active .radio-indicator {
    border-color: var(--ui-accent);
    background: var(--ui-accent);
    box-shadow: inset 0 0 0 2.5px var(--bg-primary);
  }

  .toggle-label {
    font-size: var(--size-sm);
    font-weight: 500;
  }
</style>
