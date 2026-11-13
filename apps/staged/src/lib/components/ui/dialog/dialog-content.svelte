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
  import { SHORT_VIEWPORT_PX, viewport, watchViewport } from '$lib/shared/viewport.svelte';
  import { visibleViewport, watchVisualViewport } from '$lib/shared/visualViewport.svelte';
  import { restoreFocusWithoutScroll } from '$lib/shared/restoreFocusWithoutScroll';
  import { getWindowSync } from '$lib/transport';
  import {
    findEnabledSubmitButton,
    findReturnSubmitForm,
    isDefaultActionKeydown,
  } from '$lib/shared/dialogDefaultAction';

  let {
    ref = $bindable(null),
    class: className,
    style: styleProp,
    portalProps,
    children,
    showCloseButton = true,
    fullScreenOnMobile = true,
    defaultAction,
    onpointerdown: onPointerDown,
    onkeydown: onKeydown,
    onOpenAutoFocus,
    onCloseAutoFocus,
    onInteractOutside,
    ...restProps
  }: WithoutChildrenOrChild<DialogPrimitive.ContentProps> & {
    portalProps?: WithoutChildrenOrChild<ComponentProps<typeof DialogPortal>>;
    children: Snippet;
    showCloseButton?: boolean;
    fullScreenOnMobile?: boolean;
    /**
     * What a plain Return does while focus is on the dialog rather than on
     * one of its controls: `submit` presses the submit button of the
     * dialog's form, `none` does nothing. Left unset, it submits, unless the
     * form holds a multiline editor: there Return breaks a line and
     * Cmd+Return submits, so plain Return does nothing. A dialog with no
     * form has nothing to submit either way.
     */
    defaultAction?: 'submit' | 'none';
  } = $props();

  // Keep `viewport.isMobile` live even when the host screen never subscribed.
  onMount(() => watchViewport());

  const fullScreen = $derived(fullScreenOnMobile && viewport.isMobile);

  // A full-screen dialog covers the visual viewport — the part of the page
  // actually on screen — so the on-screen keyboard shrinks it rather than
  // scrolling it away.
  $effect(() => {
    if (fullScreen) return watchVisualViewport();
  });

  // Edge-to-edge geometry applied as inline style so it cleanly overrides any
  // per-modal sizing classes the caller passes (e.g. `sm:max-w-[580px]`,
  // `max-h-[calc(100dvh-16dvh)]`) without specificity fights. Tailwind v4
  // translate utilities use the individual `translate` property, so reset that
  // alongside `transform` to prevent desktop centering from shifting the
  // mobile panel offscreen. Rows pack at the top (`align-content: start`)
  // instead of stretching over the full height, so a short form starts under
  // the header rather than spreading down the screen to where the keyboard
  // will open.
  //
  // While the visual viewport is the whole layout viewport (`height ===
  // null`: nothing takes typing, or a focused input has no keyboard over it)
  // the dialog is anchored to all four edges, so its surface covers the whole
  // layout viewport, under the home indicator included, whatever `100dvh`
  // measures. While a keyboard (or the bar iOS shows over a hardware one)
  // covers part of it, `top` and `height` track the visual viewport instead,
  // pinning the last row to the keyboard's top edge.
  //
  // The surface reaches the screen's edges; only content is inset. The
  // bottom padding stays the caller's (`p-6`, or none for `p-0` layouts) and
  // the inset goes to whichever row ends the dialog — a footer,
  // the chat composer, a scrolling body — through `--dialog-safe-bottom`, so
  // that row's own background runs under the home indicator rather than the
  // dialog's showing as a band beneath it, and a scrolling body scrolls
  // behind it. It is 0 with the keyboard up, which covers the indicator.
  // Outside a full-screen dialog the property is unset; read it as
  // `var(--dialog-safe-bottom, 0px)`.
  //
  // The other insets work the same way. The top one (the status bar),
  // `--dialog-safe-top`, goes to the first row's top padding, a header's
  // background running up under the status bar. The side ones (a phone held
  // sideways: the Dynamic Island and the rounded corners),
  // `--dialog-safe-left` and `--dialog-safe-right`, go to each full-width
  // row's inline padding, so its background runs to the screen's edges and
  // a swipe there still scrolls it. The default padding (`p-6`) includes all
  // three, as does the default Close button's position, so the two stay
  // aligned; a caller that sets its own padding (`p-0` and rows that pad
  // themselves) takes them on in those rows. All three are unset outside a
  // full-screen dialog.
  //
  // Only colours, opacity and transforms transition. The base class's
  // `duration-100` with no transition property would mean `transition: all`,
  // animating `top` and `height` behind every keyboard move; a tap on the
  // last row then landed or missed depending on whether a caller happened to
  // narrow the property (NewSessionModal's `transition-colors` did).
  //
  // `cramped`: with too little visible to fit every row, the dialog scrolls
  // rather than clip, and `data-cramped` lets its rows give way (the session
  // dialog drops its header, puts Send beside the message box and scrolls
  // its chat pane anchored at that row).
  const cramped = $derived(
    fullScreen &&
      (visibleViewport.height !== null
        ? visibleViewport.height < SHORT_VIEWPORT_PX
        : viewport.shortWindow)
  );

  const fullScreenStyle = $derived.by(() => {
    const { top, height, keyboardOpen } = visibleViewport;
    const geometry =
      height === null
        ? ['top:0', 'bottom:0', 'height:auto']
        : [`top:${top}px`, 'bottom:auto', `height:${height}px`];
    return [
      'position:fixed',
      'left:0',
      'right:0',
      ...geometry,
      'width:100%',
      'max-width:none',
      'max-height:none',
      'margin:0',
      'transform:none',
      'translate:none',
      'border-radius:0',
      'align-content:start',
      '--dialog-safe-top:env(safe-area-inset-top)',
      '--dialog-safe-left:env(safe-area-inset-left)',
      '--dialog-safe-right:env(safe-area-inset-right)',
      `--dialog-safe-bottom:${keyboardOpen ? '0px' : 'env(safe-area-inset-bottom)'}`,
      'transition-property:color,background-color,border-color,outline-color,opacity,transform,translate,scale',
      ...(cramped ? ['overflow-y:auto', 'overscroll-behavior:contain'] : []),
    ].join(';');
  });

  const contentStyle = $derived(
    fullScreen ? `${styleProp ? `${styleProp};` : ''}${fullScreenStyle}` : styleProp
  );

  // On touch screens, focus goes back to the dialog's trigger without
  // scrolling it into view; see restoreFocusWithoutScroll. With a mouse and
  // keyboard, bits-ui's restore (which shows the trigger) is kept.
  const focusRestore = restoreFocusWithoutScroll(() => viewport.hasCoarsePointer);

  /** The form a plain Return submits here, if any; see `defaultAction`. */
  function returnSubmitForm(content: HTMLElement): HTMLFormElement | null {
    if (defaultAction === 'none') return null;
    return findReturnSubmitForm(content, defaultAction === 'submit');
  }

  function handleOpenAutoFocus(event: Event) {
    focusRestore.capture();
    if (onOpenAutoFocus) {
      onOpenAutoFocus(event);
      return;
    }
    // No control is focused on open unless the caller says otherwise:
    // bits-ui would pick the first one, usually Close or Cancel.
    event.preventDefault();
    focusContentForReturn();
  }

  // A dialog that submits on Return has to hold the focus to hear the key:
  // left on the page behind, Return goes to whatever opened the dialog. The
  // dialog itself takes it, as an alert dialog does, a frame after opening
  // so that a field the dialog focuses on its own keeps it. A dialog with
  // nothing for Return to submit (a viewer, a multiline editor) is left as
  // it was, with focus where it started.
  function focusContentForReturn() {
    requestAnimationFrame(() => {
      const content = ref;
      if (!content?.isConnected || content.dataset.state !== 'open') return;
      if (content.contains(content.ownerDocument.activeElement)) return;
      if (!returnSubmitForm(content)) return;
      content.focus({ preventScroll: true });
    });
  }

  type DialogKeyboardEvent = KeyboardEvent & { currentTarget: EventTarget & HTMLDivElement };

  // A plain Return on the dialog, or on text in it, submits its form. A
  // field submits on its own and a focused button presses itself, so those
  // are left alone (see isDefaultActionKeydown) and nothing fires twice. The
  // submit button is clicked, not the handler called, so its disabled state
  // and the form's validation apply as they do for the mouse.
  function handleKeydown(event: DialogKeyboardEvent) {
    onKeydown?.(event);
    if (!isDefaultActionKeydown(event)) return;
    const form = returnSubmitForm(event.currentTarget);
    const button = form && findEnabledSubmitButton(form);
    if (!button) return;
    // Stopped as well as prevented: views under the dialog listen on the
    // window.
    event.preventDefault();
    event.stopPropagation();
    button.click();
  }

  function handleCloseAutoFocus(event: Event) {
    onCloseAutoFocus?.(event);
    focusRestore.restore(event);
  }

  /**
   * A press outside the dialog closes it, except in a toast. The toaster
   * (svelte-sonner, mounted at the app root) is portalled beside the dialog
   * rather than behind it, so a toast's Undo or its close button is part of
   * the current interaction, not a return to the page: the dialog stays open
   * and the press reaches the toast. The caller's handler runs first and can
   * prevent the close on its own grounds.
   */
  function handleInteractOutside(event: PointerEvent) {
    onInteractOutside?.(event);
    if (event.defaultPrevented) return;
    if (isToastInteraction(event)) event.preventDefault();
  }

  function isToastInteraction(event: Event): boolean {
    const target = event.target;
    return target instanceof Element && target.closest('[data-sonner-toaster]') !== null;
  }

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
  <!--
    preventOverflowTextSelection is off: bits-ui's selection lock rewrites
    user-select on <body> and the content on pointerup, and WKWebView collapses
    the just-dragged selection when that style changes — making dialog text
    unselectable in the Tauri app. The overlay already covers the page behind,
    so selection overflow has nothing visible to leak into.

    preventScroll is off too: the app shell never scrolls <body> (app.css
    sets html, body { overflow: hidden }), so bits-ui's body scroll lock has
    nothing to lock and its only effect was writing `pointer-events: none` on
    <body> at open and clearing it at close. pointer-events is inherited, so
    each write restyles the whole document — ~65 ms per open and per close
    with the home grid retained under a project (App.svelte's workspace
    layers, ~7.5k nodes) — and the overlay already keeps the pointer off the
    page behind.

    Do not extend this to menus, popovers or selects: in this codebase they
    still lock (PopperLayer resolves `preventScroll ?? true`, select-content
    defaults it on). They have no overlay, so that body `pointer-events: none`
    is what swallows the outside click that dismisses them.
  -->
  <!--
    Desktop centering is `inset: 0; margin: auto` on a fit-content height, not
    `top/left: 50%` with a -50% translate. The translate puts the dialog at a
    fractional device-pixel offset whenever its height is odd in half-pixels
    (a 16:9 video alone makes it so), and WKWebView composites the native
    `<video controls>` bar — a backdrop-filter layer — through that fractional
    transform, which smears its timeline and time labels into a blur while a
    clip plays. Layout-positioned boxes are pixel-snapped, so auto margins
    centre to the same place and stay crisp; the zoom animation only uses
    `transform`, so it still runs. The mobile full-screen style zeroes the
    margin along with the insets. Being anchored to every edge also means an
    auto width fills the viewport (to the max-width) rather than shrinking to
    the content: a dialog that should hug its content passes `w-fit`.
  -->
  <DialogPrimitive.Content
    bind:ref
    data-slot="dialog-content"
    class={cn(
      'bg-card text-card-foreground data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 ring-foreground/10 grid max-w-[calc(100%-2rem)] gap-6 rounded-xl pt-[calc(--spacing(6)+var(--dialog-safe-top,0px))] pb-6 pl-[calc(--spacing(6)+var(--dialog-safe-left,0px))] pr-[calc(--spacing(6)+var(--dialog-safe-right,0px))] text-sm ring-1 duration-100 sm:max-w-md fixed z-(--z-index-overlay) w-full outline-none',
      fullScreen
        ? 'data-open:slide-in-from-bottom data-closed:slide-out-to-bottom'
        : 'inset-0 m-auto h-fit data-closed:zoom-out-95 data-open:zoom-in-95',
      className
    )}
    style={contentStyle}
    data-full-screen={fullScreen ? '' : undefined}
    data-cramped={cramped ? '' : undefined}
    onOpenAutoFocus={handleOpenAutoFocus}
    onCloseAutoFocus={handleCloseAutoFocus}
    onInteractOutside={handleInteractOutside}
    onpointerdown={handlePointerDown}
    onkeydown={handleKeydown}
    preventOverflowTextSelection={false}
    preventScroll={false}
    {...restProps}
  >
    {@render children?.()}
    {#if showCloseButton}
      <DialogPrimitive.Close data-slot="dialog-close">
        {#snippet child({ props })}
          <Button
            variant="ghost"
            class="absolute top-[calc(--spacing(4)+var(--dialog-safe-top,0px))] right-[calc(--spacing(4)+var(--dialog-safe-right,0px))]"
            size="icon-sm"
            {...props}
          >
            <XIcon />
            <span class="sr-only">Close</span>
          </Button>
        {/snippet}
      </DialogPrimitive.Close>
    {/if}
  </DialogPrimitive.Content>
</DialogPortal>
