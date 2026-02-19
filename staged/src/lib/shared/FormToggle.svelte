<script lang="ts" generics="T extends string">
  interface Props {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    options: { value: T; label: string; description?: string; icon?: any }[];
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
      {#if option.icon}
        <span class="toggle-icon">
          <option.icon size={22} />
        </span>
      {/if}
      <span class="toggle-text">
        <span class="toggle-label">{option.label}</span>
        {#if option.description}
          <span class="toggle-desc">{option.description}</span>
        {/if}
      </span>
    </button>
  {/each}
</div>

<style>
  .toggle-group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .toggle-card {
    display: flex;
    align-items: center;
    gap: 12px;
    border: 1.5px solid var(--border-muted);
    border-radius: 10px;
    background: transparent;
    color: var(--text-muted);
    padding: 14px 16px;
    font-family: inherit;
    cursor: pointer;
    text-align: left;
    transition:
      border-color 0.2s ease,
      color 0.2s ease;
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

  .toggle-icon {
    display: flex;
    flex-shrink: 0;
    color: var(--text-faint);
    transition: color 0.2s ease;
  }

  .toggle-card.active .toggle-icon {
    color: var(--ui-accent);
  }

  .toggle-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .toggle-label {
    font-size: var(--size-sm);
    font-weight: 500;
  }

  .toggle-desc {
    font-size: var(--size-xs);
    color: var(--text-faint);
    font-weight: 400;
  }

  .toggle-card.active .toggle-desc {
    color: var(--text-muted);
  }
</style>
