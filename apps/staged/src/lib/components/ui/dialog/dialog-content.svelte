<script lang="ts">
  import { Dialog as DialogPrimitive } from 'bits-ui';
  import DialogPortal from './dialog-portal.svelte';
  import type { Snippet } from 'svelte';
  import { onMount } from 'svelte';
  import * as Dialog from './index.js';
  import { cn, type WithoutChildrenOrChild } from '$lib/components/utils.js';
  import type { ComponentProps } from 'svelte';
  import { Button } from '$lib/components/ui/button/index.js';
  import XIcon from '@lucide/svelte/icons/x';
  import { viewport, watchViewport } from '$lib/shared/viewport.svelte';
  import { watchKeyboardInset } from '$lib/shared/keyboardInset.svelte';
  import { getWindowSync } from '$lib/transport';

  let {
    ref = $bindable(null),
    class: className,
    style: styleProp,
    portalProps,
    children,
    showCloseButton = true,
    fullScreenOnMobile = true,
    onpointerdown: onPointerDown,
    ...restProps
  }: WithoutChildrenOrChild<DialogPrimitive.ContentProps> & {
    portalProps?: WithoutChildrenOrChild<ComponentProps<typeof DialogPortal>>;
    children: Snippet;
    showCloseButton?: boolean;
    fullScreenOnMobile?: boolean;
  } = $props();

  // Keep `viewport.isMobile` live even when the host screen never subscribed,
  // and keep `--keyboard-inset` updated so the full-screen height shrinks to
  // the space above the on-screen keyboard.
  onMount(() => {
    const unwatchViewport = watchViewport();
    const unwatchKeyboard = watchKeyboardInset();
    return () => {
      unwatchViewport();
      unwatchKeyboard();
    };
  });

  const fullScreen = $derived(fullScreenOnMobile && viewport.isMobile);

  // Edge-to-edge geometry applied as inline style so it cleanly overrides any
  // per-modal sizing classes the caller passes (e.g. `sm:max-w-[580px]`,
  // `max-h-[calc(100vh-16vh)]`) without specificity fights. Safe-area padding
  // keeps the header/footer clear of notches and home indicators. Tailwind v4
  // translate utilities use the individual `translate` property, so reset that
  // alongside `transform` to prevent desktop centering from shifting the mobile
  // panel offscreen. The height subtracts `--keyboard-inset` so the dialog
  // shrinks to the space above the on-screen keyboard, pinning its footer to
  // the keyboard's top edge.
  const fullScreenStyle =
    'position:fixed;inset:0;top:0;left:0;width:100%;max-width:none;height:calc(100dvh - var(--keyboard-inset, 0px));max-height:none;transform:none;translate:none;border-radius:0;padding-top:env(safe-area-inset-top);padding-bottom:env(safe-area-inset-bottom);';

  const contentStyle = $derived(
    fullScreen ? `${styleProp ? `${styleProp};` : ''}${fullScreenStyle}` : styleProp
  );

  const dragRegionSelector =
    '[data-dialog-drag-region], [data-slot="dialog-header"], .modal-header, .diagram-viewer-header';
  const noDragSelector =
    'button, a, input, select, textarea, summary, [role="button"], [role="link"], [role="textbox"], [contenteditable="true"], [data-dialog-no-drag]';

  type DialogPointerEvent = PointerEvent & { currentTarget: EventTarget & HTMLDivElement };

  function startFullScreenDrag(event: DialogPointerEvent) {
    if (!fullScreen || event.button !== 0) return;

    const contentElement = event.currentTarget;
    const target = event.target as HTMLElement | null;
    if (!target || !contentElement.contains(target)) return;
    if (target.closest(noDragSelector)) return;

    const dragRegion = target.closest(dragRegionSelector);
    if (!dragRegion || !contentElement.contains(dragRegion)) return;

    event.preventDefault();
    void getWindowSync().startDragging();
  }

  function handlePointerDown(event: DialogPointerEvent) {
    onPointerDown?.(event);
    if (!event.defaultPrevented) {
      startFullScreenDrag(event);
    }
  }
</script>

<DialogPortal {...portalProps}>
  <Dialog.Overlay />
  <DialogPrimitive.Content
    bind:ref
    data-slot="dialog-content"
    class={cn(
      'bg-card text-card-foreground data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 ring-foreground/10 grid max-w-[calc(100%-2rem)] gap-6 rounded-xl p-6 text-sm ring-1 duration-100 sm:max-w-md fixed z-(--z-index-overlay) w-full outline-none',
      fullScreen
        ? 'data-open:slide-in-from-bottom data-closed:slide-out-to-bottom'
        : 'top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 data-closed:zoom-out-95 data-open:zoom-in-95',
      className
    )}
    style={contentStyle}
    onOpenAutoFocus={(e) => e.preventDefault()}
    onpointerdown={handlePointerDown}
    {...restProps}
  >
    {@render children?.()}
    {#if showCloseButton}
      <DialogPrimitive.Close data-slot="dialog-close">
        {#snippet child({ props })}
          <Button variant="ghost" class="absolute top-4 right-4" size="icon-sm" {...props}>
            <XIcon />
            <span class="sr-only">Close</span>
          </Button>
        {/snippet}
      </DialogPrimitive.Close>
    {/if}
  </DialogPrimitive.Content>
</DialogPortal>
