<script lang="ts">
  import { Dialog as DialogPrimitive } from 'bits-ui';
  import { cn } from '$lib/components/utils.js';

  let {
    ref = $bindable(null),
    class: className,
    ...restProps
  }: DialogPrimitive.OverlayProps = $props();
</script>

<DialogPrimitive.Overlay
  bind:ref
  data-slot="dialog-overlay"
  class={cn(
    'data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 bg-[var(--shadow-overlay)] duration-100 supports-backdrop-filter:backdrop-blur-xs fixed inset-0 isolate z-overlay',
    className
  )}
  {...restProps}
/>
