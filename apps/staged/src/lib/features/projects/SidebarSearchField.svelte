<!--
  SidebarSearchField.svelte - Inset search well for the projects sidebar.

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
  <Search size={14} />
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
      <X size={14} />
    </button>
  {/if}
</label>

<style>
  /* A tinted well like the sidebar's row hover at rest; on focus it lifts onto
     the input surface (white in light mode, raised in dark) with an outline.
     30px tall and 8px round to match the New project pill and repo cards. */
  .search-field {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    height: 30px;
    padding: 0 10px;
    border-radius: 8px;
    background: var(--projects-sidebar-field-bg);
    color: var(--text-muted);
    cursor: text;
    transition:
      background-color 0.15s ease,
      box-shadow 0.15s ease;
  }

  .search-field:hover {
    background: var(--projects-sidebar-field-bg-hover);
  }

  .search-field:focus-within {
    background: var(--bg-input);
    box-shadow: inset 0 0 0 1px var(--border-emphasis);
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
    color: var(--text-muted);
  }

  /* 22px hit target; the negative margin puts the ✕ glyph 10px from the
     edge, mirroring the magnifier. */
  .clear {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 22px;
    height: 22px;
    margin-right: -4px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    padding: 0;
  }

  .clear:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }
</style>
