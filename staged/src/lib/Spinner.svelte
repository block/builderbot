<script lang="ts">
  import { Loader2, RefreshCw } from 'lucide-svelte';
  import type { ComponentType } from 'svelte';

  let {
    size = 16,
    icon = 'loader' as 'loader' | 'refresh',
    class: className = '',
  }: {
    size?: number;
    icon?: 'loader' | 'refresh';
    class?: string;
  } = $props();

  const icons: Record<'loader' | 'refresh', ComponentType> = {
    loader: Loader2,
    refresh: RefreshCw,
  };

  const IconComponent = $derived(icons[icon]);
</script>

<IconComponent {size} class="spinner {className}" />

<style>
  :global(.spinner) {
    animation: spin 1s linear infinite;
    flex-shrink: 0;
    /* Prevent wobbling by ensuring the icon is centered and uses transform-origin */
    display: inline-block;
    transform-origin: center center;
    will-change: transform;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
