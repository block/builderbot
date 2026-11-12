<!--
  TagIcon.svelte — a tag's icon in its colour, or a coloured dot for a tag
  with no icon.

  The legacy built-in statuses' icons are statically imported so they paint
  on the first frame; any other name goes through the lazy full icon map (see
  NamedIcon), showing CircleDot until it lands. Renders the bare SVG so
  parents keep control of layout.
-->
<script lang="ts">
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import CircleDot from '@lucide/svelte/icons/circle-dot';
  import CirclePause from '@lucide/svelte/icons/circle-pause';
  import Eye from '@lucide/svelte/icons/eye';
  import Lightbulb from '@lucide/svelte/icons/lightbulb';
  import OctagonMinus from '@lucide/svelte/icons/octagon-minus';
  import Paintbrush from '@lucide/svelte/icons/paintbrush';
  import NamedIcon from '../actions/NamedIcon.svelte';
  import type { IconComponent } from '../actions/lucideIcons';
  import type { Tag } from '../../types';
  import { TAG_COLOR_VARS } from './tagColors';

  const BUILT_IN_ICONS: Record<string, IconComponent> = {
    'circle-check': CircleCheck,
    'circle-dot': CircleDot,
    'circle-pause': CirclePause,
    eye: Eye,
    lightbulb: Lightbulb,
    'octagon-minus': OctagonMinus,
    paintbrush: Paintbrush,
  };

  interface Props {
    tag: Pick<Tag, 'icon' | 'color'>;
    size?: number;
  }

  let { tag, size = 14 }: Props = $props();

  let BuiltIn = $derived(tag.icon ? BUILT_IN_ICONS[tag.icon] : undefined);
  let style = $derived(`color: ${TAG_COLOR_VARS[tag.color]}`);
</script>

{#if tag.icon === null}
  <svg
    class="tag-dot"
    width={size}
    height={size}
    viewBox="0 0 24 24"
    {style}
    aria-hidden="true"
    focusable="false"
  >
    <circle cx="12" cy="12" r="5" fill="currentColor" />
  </svg>
{:else if BuiltIn}
  <BuiltIn {size} {style} />
{:else}
  <NamedIcon name={tag.icon} fallback={CircleDot} {size} {style} />
{/if}

<style>
  .tag-dot {
    flex-shrink: 0;
  }
</style>
