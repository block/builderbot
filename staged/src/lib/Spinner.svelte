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
    display: inline-block;
    /* Center the rotation precisely */
    transform-origin: center center;
    /* Force GPU acceleration and create own compositing layer */
    transform: translate3d(0, 0, 0);
    will-change: transform;
    backface-visibility: hidden;
    /* Prevent subpixel wobble */
    -webkit-font-smoothing: antialiased;
    -moz-osx-font-smoothing: grayscale;
    /* Ensure the element stays in its own layer */
    isolation: isolate;
    /* Round to whole pixels */
    transform-box: fill-box;
  }

  @keyframes spin {
    from {
      transform: translate3d(0, 0, 0) rotate(0deg);
    }
    to {
      transform: translate3d(0, 0, 0) rotate(360deg);
    }
  }
</style>
