<!--
  TagChip.svelte — a tag as a small rounded chip: its name (and icon, if it
  has one) in its colour over a faint wash of the same colour, so a chip needs
  no colours of its own and follows the theme.

  With `onRemove`, a trailing × removes it (the header's ungrouped tags).
-->
<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import type { Tag } from '../../types';
  import TagIcon from './TagIcon.svelte';
  import { TAG_COLOR_VARS } from './tagColors';

  interface Props {
    tag: Pick<Tag, 'name' | 'color' | 'icon'>;
    onRemove?: () => void;
    /** `sm` for cards, `md` for the project header. */
    size?: 'sm' | 'md';
    disabled?: boolean;
  }

  let { tag, onRemove, size = 'md', disabled = false }: Props = $props();

  let iconSize = $derived(size === 'sm' ? 10 : 12);
</script>

<span
  class="tag-chip"
  class:sm={size === 'sm'}
  class:removable={!!onRemove}
  style:--tag-chip-color={TAG_COLOR_VARS[tag.color]}
  title={tag.name}
>
  {#if tag.icon !== null}
    <TagIcon {tag} size={iconSize} />
  {/if}
  <span class="tag-chip-name">{tag.name}</span>
  {#if onRemove}
    <button
      type="button"
      class="tag-chip-remove"
      aria-label={`Remove tag ${tag.name}`}
      title={`Remove ${tag.name}`}
      {disabled}
      onclick={onRemove}
    >
      <X size={iconSize} />
    </button>
  {/if}
</span>

<style>
  .tag-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 1;
    min-width: 0;
    max-width: 180px;
    padding: 1px 8px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--tag-chip-color) 16%, transparent);
    color: var(--tag-chip-color);
    font-size: var(--size-xs);
    font-weight: 600;
    line-height: 1.5;
  }

  .tag-chip.sm {
    max-width: 120px;
    padding: 0 6px;
    gap: 3px;
    font-size: calc(var(--size-xs) - 1px);
  }

  .tag-chip.removable {
    padding-right: 3px;
  }

  .tag-chip :global(svg) {
    flex-shrink: 0;
  }

  .tag-chip-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tag-chip-remove {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    padding: 2px;
    border: none;
    border-radius: 999px;
    background: transparent;
    color: inherit;
    opacity: 0.7;
    cursor: pointer;
  }

  .tag-chip-remove:hover:not(:disabled) {
    opacity: 1;
    background: color-mix(in srgb, var(--tag-chip-color) 20%, transparent);
  }

  .tag-chip-remove:disabled {
    cursor: default;
    opacity: 0.4;
  }
</style>
