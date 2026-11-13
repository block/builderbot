<script lang="ts">
  import { AlertDialog as AlertDialogPrimitive } from 'bits-ui';
  import AlertDialogPortal from './alert-dialog-portal.svelte';
  import AlertDialogOverlay from './alert-dialog-overlay.svelte';
  import { cn, type WithoutChild, type WithoutChildrenOrChild } from '$lib/components/utils.js';
  import type { ComponentProps } from 'svelte';
  import { onMount } from 'svelte';
  import { viewport, watchViewport } from '$lib/shared/viewport.svelte';
  import { restoreFocusWithoutScroll } from '$lib/shared/restoreFocusWithoutScroll';
  import { findEnabledButton, isDefaultActionKeydown } from '$lib/shared/dialogDefaultAction';

  /** Which button a plain Return presses; see `defaultAction`. */
  type DefaultAction = 'action' | 'cancel' | 'none';

  let {
    ref = $bindable(null),
    class: className,
    size = 'default',
    portalProps,
    onOpenAutoFocus,
    onCloseAutoFocus,
    defaultAction,
    onkeydown,
    ...restProps
  }: WithoutChild<AlertDialogPrimitive.ContentProps> & {
    size?: 'default' | 'sm';
    /**
     * The button a plain Return presses while focus is on the dialog rather
     * than on one of its controls: the Action, the Cancel, or neither. Left
     * unset, it is the Action, unless that is destructive: Return never
     * confirms a destructive action by default, so there it does nothing.
     */
    defaultAction?: DefaultAction;
    portalProps?: WithoutChildrenOrChild<ComponentProps<typeof AlertDialogPortal>>;
  } = $props();

  onMount(() => watchViewport());

  // Focus goes back to the trigger without scrolling on touch screens, as in
  // dialog-content.
  const focusRestore = restoreFocusWithoutScroll(() => viewport.hasCoarsePointer);

  function handleOpenAutoFocus(event: Event) {
    focusRestore.capture();
    onOpenAutoFocus?.(event);
  }

  function handleCloseAutoFocus(event: Event) {
    onCloseAutoFocus?.(event);
    focusRestore.restore(event);
  }

  // The dialog opens with focus on its own container, where no button hears
  // Return. The default button is clicked, not called, so its disabled state
  // and click handler apply as they do for the mouse.
  function handleKeydown(event: KeyboardEvent & { currentTarget: EventTarget & HTMLDivElement }) {
    onkeydown?.(event);
    if (!isDefaultActionKeydown(event)) return;
    const content = event.currentTarget;
    const action = content.querySelector<HTMLElement>('[data-slot="alert-dialog-action"]');
    const which: DefaultAction =
      defaultAction ?? (action?.dataset.variant === 'destructive' ? 'none' : 'action');
    if (which === 'none') return;
    const button = findEnabledButton(content, `[data-slot="alert-dialog-${which}"]`);
    if (!button) return;
    // Stopped as well as prevented: views under the dialog listen on the
    // window (the splash form creates a project on Return).
    event.preventDefault();
    event.stopPropagation();
    button.click();
  }
</script>

<AlertDialogPortal {...portalProps}>
  <AlertDialogOverlay />
  <!--
    preventOverflowTextSelection is off for the same reason as dialog-content:
    WKWebView collapses the selection when bits-ui restores user-select styles
    on pointerup, leaving dialog text unselectable. preventScroll is off for
    the same reason as there: <body> never scrolls and the lock's
    `pointer-events: none` write on it restyles the whole document. Menus,
    popovers and selects are deliberately left locking (see dialog-content).
  -->
  <AlertDialogPrimitive.Content
    bind:ref
    data-slot="alert-dialog-content"
    data-size={size}
    class={cn(
      'data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 bg-[var(--bg-alert-dialog)] text-[var(--text-primary)] ring-foreground/10 gap-6 rounded-xl p-6 ring-1 duration-100 data-[size=default]:max-w-xs data-[size=sm]:max-w-xs data-[size=default]:sm:max-w-lg group/alert-dialog-content fixed top-1/2 left-1/2 z-(--z-index-overlay) grid w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 outline-none',
      className
    )}
    onOpenAutoFocus={handleOpenAutoFocus}
    onCloseAutoFocus={handleCloseAutoFocus}
    onkeydown={handleKeydown}
    preventOverflowTextSelection={false}
    preventScroll={false}
    {...restProps}
  />
</AlertDialogPortal>
