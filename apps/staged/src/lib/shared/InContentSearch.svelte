<!--
  InContentSearch.svelte — Reusable in-content search bar

  Provides a compact search UI with keyboard navigation support.
  Displays match counter and navigation buttons.
-->
<script lang="ts">
  import ChevronUp from '@lucide/svelte/icons/chevron-up';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import X from '@lucide/svelte/icons/x';
  import { tick } from 'svelte';
  import { viewport } from './viewport.svelte';
  import { Input } from '$lib/components/ui/input';
  import { Button } from '$lib/components/ui/button';
  import * as Tooltip from '$lib/components/ui/tooltip';

  interface Props {
    visible: boolean;
    matchCount: number;
    currentIndex: number;
    onSearch: (query: string) => void;
    onNext: () => void;
    onPrevious: () => void;
    onClose: () => void;
  }

  let { visible, matchCount, currentIndex, onSearch, onNext, onPrevious, onClose }: Props =
    $props();

  let inputEl = $state<HTMLInputElement | null>(null);
  let query = $state('');

  // Auto-focus input when search becomes visible
  $effect(() => {
    if (visible) {
      tick().then(() => inputEl?.focus());
    }
  });

  // Notify parent when query changes
  $effect(() => {
    onSearch(query);
  });

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (e.shiftKey) {
        onPrevious();
      } else {
        onNext();
      }
    }
  }

  // Display match counter (1-based for users)
  let counterText = $derived.by(() => {
    if (!query.trim()) return '';
    if (matchCount === 0) return 'No matches';
    return `${currentIndex + 1} of ${matchCount}`;
  });
</script>

{#if visible}
  <div class="search-bar">
    <Input
      bind:ref={inputEl}
      bind:value={query}
      type="text"
      placeholder="Search..."
      onkeydown={handleKeydown}
      class="border-0 bg-transparent shadow-none px-0 py-0 h-auto min-h-0 focus-visible:ring-0 focus-visible:border-0 md:text-base w-[200px]"
    />
    {#if counterText}
      <span class="match-counter">{counterText}</span>
    {/if}
    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon-xs"
            class="[&_svg]:size-3.5"
            onclick={onPrevious}
            disabled={matchCount === 0}
          >
            <ChevronUp size={14} />
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>
        {viewport.showShortcutHints ? 'Previous match (Shift+Enter)' : 'Previous match'}
      </Tooltip.Content>
    </Tooltip.Root>
    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon-xs"
            class="[&_svg]:size-3.5"
            onclick={onNext}
            disabled={matchCount === 0}
          >
            <ChevronDown size={14} />
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>
        {viewport.showShortcutHints ? 'Next match (Enter)' : 'Next match'}
      </Tooltip.Content>
    </Tooltip.Root>
    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon-xs"
            class="ml-1 [&_svg]:size-3.5"
            onclick={onClose}
          >
            <X size={14} />
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>
        {viewport.showShortcutHints ? 'Close search (Esc)' : 'Close search'}
      </Tooltip.Content>
    </Tooltip.Root>
  </div>
{/if}

<style>
  .search-bar {
    position: absolute;
    top: 16px;
    right: 56px;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    z-index: 10;
  }

  .match-counter {
    font-size: var(--size-xs);
    color: var(--text-muted);
    white-space: nowrap;
    min-width: 60px;
    text-align: center;
  }
</style>
