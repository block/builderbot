<!--
  RepoBadge – colored pill showing a repo's short name.

  Uses the stored hue with mode-appropriate S/L values, adapting to
  the current theme via the reactive darkMode signal.
-->
<script lang="ts">
  import { darkMode } from '../stores/isDark.svelte';

  interface Props {
    shortName: string;
    hue: number;
    small?: boolean;
  }

  let { shortName, hue, small = false }: Props = $props();

  let bg = $derived(darkMode.value ? `hsl(${hue} 35% 22%)` : `hsl(${hue} 50% 92%)`);

  let fg = $derived(darkMode.value ? `hsl(${hue} 50% 75%)` : `hsl(${hue} 55% 35%)`);
</script>

<span class="repo-badge" class:small style="background: {bg}; color: {fg};" title={shortName}>
  {shortName}
</span>

<style>
  .repo-badge {
    display: inline-flex;
    align-items: center;
    padding: 1px 5px;
    border-radius: 4px;
    font-size: 11px;
    font-weight: 600;
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    line-height: 1.4;
    white-space: nowrap;
  }

  .repo-badge.small {
    padding: 0px 4px;
    font-size: 9.5px;
    border-radius: 3px;
    line-height: 1.3;
  }
</style>
