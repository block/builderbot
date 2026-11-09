<script lang="ts">
  import { cn, type WithElementRef } from '$lib/components/utils.js';
  import type { HTMLAttributes } from 'svelte/elements';
  import { Dialog as DialogPrimitive } from 'bits-ui';
  import { Button } from '$lib/components/ui/button/index.js';

  let {
    ref = $bindable(null),
    class: className,
    children,
    showCloseButton = false,
    ...restProps
  }: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
    showCloseButton?: boolean;
  } = $props();
</script>

<!-- The bottom padding keeps the buttons clear of the home indicator when
     the footer ends a full-screen dialog (see dialog-content); it is 0
     everywhere else. -->
<div
  bind:this={ref}
  data-slot="dialog-footer"
  class={cn(
    'gap-2 flex flex-col-reverse gap-2 pb-[var(--dialog-safe-bottom,0px)] sm:flex-row sm:justify-end',
    className
  )}
  {...restProps}
>
  {@render children?.()}
  {#if showCloseButton}
    <DialogPrimitive.Close>
      {#snippet child({ props })}
        <Button variant="outline" {...props}>Close</Button>
      {/snippet}
    </DialogPrimitive.Close>
  {/if}
</div>
