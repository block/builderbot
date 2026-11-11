<!--
  TagColorSwatches.svelte — the seven palette colours as a radio group of
  dots, for a tag row in Settings › Tags.
-->
<script lang="ts">
  import type { TagColor } from '../../../types';
  import { TAG_COLORS, TAG_COLOR_VARS } from '../../tags/tagColors';

  interface Props {
    color: TagColor;
    label?: string;
    onSelect: (color: TagColor) => void;
  }

  let { color, label = 'Tag color', onSelect }: Props = $props();
</script>

<div class="swatches" role="radiogroup" aria-label={label}>
  {#each TAG_COLORS as option (option)}
    <button
      type="button"
      class="swatch"
      class:selected={color === option}
      role="radio"
      aria-checked={color === option}
      aria-label={option}
      title={option}
      style:--swatch-color={TAG_COLOR_VARS[option]}
      onclick={() => {
        if (option !== color) onSelect(option);
      }}
    ></button>
  {/each}
</div>

<style>
  .swatches {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .swatch {
    width: 16px;
    height: 16px;
    padding: 0;
    border: 2px solid transparent;
    border-radius: 50%;
    background: var(--swatch-color);
    background-clip: content-box;
    cursor: pointer;
    transition: border-color 0.1s ease;
  }

  .swatch:hover {
    border-color: var(--border-emphasis);
  }

  .swatch.selected {
    border-color: var(--swatch-color);
    padding: 2px;
  }

  /* On a touch screen each swatch gets a 30px box; the dot stays 12px, drawn
     inside the padding. */
  @media (pointer: coarse) {
    .swatch {
      width: 30px;
      height: 30px;
      padding: 7px;
    }

    .swatch.selected {
      padding: 9px;
    }
  }
</style>
