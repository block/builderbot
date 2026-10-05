<!--
  ProjectStatusOptionIcon.svelte — the icon a project status option picked.

  The built-in statuses' icons are statically imported so they paint on the
  first frame; any other name goes through the lazy full icon map (see
  NamedIcon), showing CircleDot until it lands.
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
  import { PROJECT_STATUS_COLOR_VARS, type ProjectStatusOption } from './projectStatusDisplay';

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
    option: Pick<ProjectStatusOption, 'icon' | 'color'>;
    size?: number;
  }

  let { option, size = 14 }: Props = $props();

  let BuiltIn = $derived(BUILT_IN_ICONS[option.icon]);
  let style = $derived(`color: ${PROJECT_STATUS_COLOR_VARS[option.color]}`);
</script>

{#if BuiltIn}
  <BuiltIn {size} {style} />
{:else}
  <NamedIcon name={option.icon} fallback={CircleDot} {size} {style} />
{/if}
