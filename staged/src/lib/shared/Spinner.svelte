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

<div class="spinner-container {className}">
  <IconComponent {size} />
</div>

<style>
  .spinner-container {
    animation: spin 1s linear infinite;
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transform-origin: center center;
    will-change: transform;
  }

  :global {
    @keyframes spin {
      from {
        transform: rotate(0deg);
      }
      to {
        transform: rotate(360deg);
      }
    }
  }
</style>
