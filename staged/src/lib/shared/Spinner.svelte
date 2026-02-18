<script lang="ts">
  import { Loader2, RefreshCw } from 'lucide-svelte';

  let {
    size = 16,
    icon = 'loader' as 'loader' | 'refresh',
    class: className = '',
  }: {
    size?: number;
    icon?: 'loader' | 'refresh';
    class?: string;
  } = $props();
</script>

<div class="spinner {className}">
  {#if icon === 'loader'}
    <Loader2 {size} />
  {:else}
    <RefreshCw {size} />
  {/if}
</div>

<style>
  .spinner {
    display: inline-block;
    animation: spin 1s linear infinite;
  }

  .spinner :global(svg) {
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
