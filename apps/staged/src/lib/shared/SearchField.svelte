<!--
  SearchField.svelte - Bordered search input shell shared by the list views

  Wraps the shared Input in the rounded, elevated box with a leading search
  icon that ProjectsList and ReposListView both use, so the styling and
  accessibility of the two stay in step. The parent owns the query (bind:value)
  and may bind `ref` to the underlying input for focus management and keyboard
  handling.

  With `clearable`, a ghost clear button appears while the query is non-empty;
  clearing empties the query and returns focus to the input. The native WebKit
  cancel glyph is suppressed either way so the field looks the same across
  engines.
-->
<script lang="ts">
  import Search from '@lucide/svelte/icons/search';
  import X from '@lucide/svelte/icons/x';
  import { Input } from '$lib/components/ui/input';
  import { Button } from '$lib/components/ui/button';

  interface Props {
    value: string;
    ref?: HTMLInputElement | null;
    /** Visible placeholder and, unless `label` is given, the accessible name. */
    placeholder: string;
    /** Accessible name for the input; defaults to the placeholder. */
    label?: string;
    clearable?: boolean;
  }

  let {
    value = $bindable(''),
    ref = $bindable(null),
    placeholder,
    label = placeholder,
    clearable = false,
  }: Props = $props();

  let showClear = $derived(clearable && value.trim().length > 0);

  function clear() {
    value = '';
    ref?.focus();
  }
</script>

<div class="search-field">
  <Search size={14} />
  <Input
    bind:ref
    bind:value
    type="search"
    {placeholder}
    aria-label={label}
    autocomplete="off"
    autocorrect="off"
    spellcheck="false"
    class="border-0 bg-transparent shadow-none px-0 py-0 h-auto min-h-0 focus-visible:ring-0 focus-visible:border-0 text-sm"
  />
  {#if showClear}
    <Button
      variant="ghost"
      size="icon-xs"
      class="-my-1 -mr-1 size-6 [&_svg]:size-3.5"
      aria-label="Clear search"
      title="Clear search"
      onclick={clear}
    >
      <X size={14} />
    </Button>
  {/if}
</div>

<style>
  .search-field {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: var(--bg-elevated);
    color: var(--text-muted);
    transition: border-color 0.15s ease;
  }

  .search-field:focus-within {
    border-color: var(--border-emphasis);
  }

  /* The explicit clear button replaces WebKit's native search cancel glyph. */
  .search-field :global(input[type='search'])::-webkit-search-cancel-button {
    -webkit-appearance: none;
    appearance: none;
  }
</style>
