<!--
  SidebarSearchField.svelte - Hairline search field for the projects sidebar.

  One field for both sidebar segments (pinned repos and projects); the owner
  binds `value` (the grid's shared projectsListViewState.searchQuery) and does
  the filtering. ✕ and Escape clear the query; the field itself holds no other
  state.
-->
<script lang="ts">
  import Search from '@lucide/svelte/icons/search';
  import X from '@lucide/svelte/icons/x';

  interface Props {
    value?: string;
    placeholder?: string;
  }

  let { value = $bindable(''), placeholder = 'Search repos & projects' }: Props = $props();

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape' && value) {
      e.preventDefault();
      value = '';
    }
  }
</script>

<label class="search-field" class:has-value={value.length > 0}>
  <Search size={13} />
  <input
    bind:value
    type="text"
    {placeholder}
    aria-label={placeholder}
    autocomplete="off"
    spellcheck="false"
    onkeydown={handleKeydown}
  />
  {#if value}
    <button type="button" class="clear" aria-label="Clear search" onclick={() => (value = '')}>
      <X size={12} />
    </button>
  {/if}
</label>

<style>
  .search-field {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
    padding: 6px 2px;
    border-bottom: 1px solid color-mix(in srgb, var(--border-subtle) 70%, transparent);
    color: var(--text-muted);
    cursor: text;
    transition: border-color 0.15s ease;
  }

  .search-field:focus-within {
    border-bottom-color: var(--border-emphasis);
  }

  .search-field :global(svg) {
    flex-shrink: 0;
  }

  input {
    flex: 1;
    min-width: 0;
    border: none;
    background: transparent;
    color: var(--text-primary);
    font: inherit;
    font-size: var(--size-sm);
    outline: none;
    padding: 0;
  }

  input::placeholder {
    color: var(--text-faint);
  }

  .clear {
    display: inline-flex;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    padding: 2px;
  }

  .clear:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }
</style>
