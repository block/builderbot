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

<div class="spinner-container {className}" style="width: {size}px; height: {size}px;">
  <div class="spinner-positioner">
    <div class="spinner-rotator">
      <IconComponent {size} />
    </div>
  </div>
</div>

<style>
  .spinner-container {
    flex-shrink: 0;
    position: relative;
  }

  .spinner-positioner {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 100%;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .spinner-rotator {
    animation: spin 1s linear infinite;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .spinner-rotator :global(svg) {
    display: block;
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
