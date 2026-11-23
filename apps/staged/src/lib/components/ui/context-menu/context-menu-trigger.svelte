<script lang="ts">
  import { ContextMenu as ContextMenuPrimitive } from 'bits-ui';
  import { onDestroy } from 'svelte';
  import { cn } from '$lib/components/utils.js';
  import { createLongPress } from './longPress.js';

  let {
    ref = $bindable(null),
    class: className,
    disabled = false,
    oncontextmenu,
    onpointerdown,
    ontouchstart,
    ...restProps
  }: ContextMenuPrimitive.TriggerProps = $props();

  // Touch long presses are ours (see longPress.ts): the mature press is
  // re-dispatched as a `contextmenu` at the touched element so consumer
  // handlers and bits-ui's own opener run exactly as for a right-click, and
  // the release that follows can no longer click the item under the finger.
  // Only the touchstart is wired here; the controller follows the rest of the
  // gesture on the document.
  const longPress = createLongPress({
    isDisabled: () => disabled,
    trigger: () => ref,
    onLongPress: ({ target, clientX, clientY }) => {
      target.dispatchEvent(
        new MouseEvent('contextmenu', {
          bubbles: true,
          cancelable: true,
          clientX,
          clientY,
          view: window,
        })
      );
    },
  });
  onDestroy(longPress.destroy);
</script>

<!-- `-webkit-touch-callout: none` + `select-none` keep iOS from raising its
     own callout/selection on the hold; both inherit through the
     `display: contents` wrapper most triggers render as. -->
<ContextMenuPrimitive.Trigger
  bind:ref
  {disabled}
  data-slot="context-menu-trigger"
  class={cn('cn-context-menu-trigger select-none [-webkit-touch-callout:none]', className)}
  oncontextmenu={(e) => {
    // Ours first: a trusted contextmenu trailing our synthetic one is eaten
    // before the consumer handler runs again, and bits-ui's own opener
    // (merged after these props) bails on the defaultPrevented event.
    if (longPress.oncontextmenu(e)) return;
    oncontextmenu?.(e);
  }}
  onpointerdown={(e) => {
    onpointerdown?.(e);
    // bits-ui runs a touch long press of its own: a 700 ms timer started here
    // that opens the menu unless a pointermove, pointerup or pointercancel
    // reaches the trigger first. Ours fires sooner and opens the menu through
    // `contextmenu`, but when ours declines or drops a press (a finger
    // catching a moving list, a scroll under a still finger) nothing would
    // stop theirs. Its handler is merged after these props and is skipped for
    // a default-prevented event, so its timer never starts. Mouse presses
    // start no timer and keep their default (focus, text selection, drag).
    if (!disabled && e.pointerType !== 'mouse') e.preventDefault();
  }}
  ontouchstart={(e) => {
    ontouchstart?.(e);
    longPress.ontouchstart(e);
  }}
  {...restProps}
/>
