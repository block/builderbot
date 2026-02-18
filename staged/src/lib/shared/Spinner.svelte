<script lang="ts">
  import { RefreshCw } from 'lucide-svelte';

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

<div class="spinner {className}" aria-hidden="true">
  {#if icon === 'loader'}
    <svg
      class="spinner-svg spinner-loader"
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <g>
        <path d="M21 12a9 9 0 1 1-6.219-8.56" />
        <animateTransform
          attributeName="transform"
          attributeType="XML"
          type="rotate"
          from="0 12 12"
          to="360 12 12"
          dur="1s"
          repeatCount="indefinite"
        />
      </g>
    </svg>
  {:else}
    <RefreshCw {size} class="spinner-svg spinner-refresh" />
  {/if}
</div>

<style>
  .spinner {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    line-height: 0;
  }

  .spinner-svg {
    display: block;
    overflow: visible;
    transform-box: view-box;
    transform-origin: 50% 50%;
    backface-visibility: hidden;
  }

  .spinner :global(.spinner-refresh) {
    animation: spin 1s linear infinite;
    will-change: transform;
  }

  @keyframes spin {
    from {
      transform: translateZ(0) rotate(0deg);
    }
    to {
      transform: translateZ(0) rotate(360deg);
    }
  }
</style>
