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
    ontouchstart,
    ontouchmove,
    ontouchend,
    ontouchcancel,
    ...restProps
  }: ContextMenuPrimitive.TriggerProps = $props();

  // Touch long presses are ours (see longPress.ts): the mature press is
  // re-dispatched as a `contextmenu` at the touched element so consumer
  // handlers and bits-ui's own opener run exactly as for a right-click, and
  // the release that follows can no longer click the item under the finger.
  const longPress = createLongPress({
    isDisabled: () => disabled,
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
    oncontextmenu?.(e);
    longPress.oncontextmenu(e);
  }}
  ontouchstart={(e) => {
    ontouchstart?.(e);
    longPress.ontouchstart(e);
  }}
  ontouchmove={(e) => {
    ontouchmove?.(e);
    longPress.ontouchmove(e);
  }}
  ontouchend={(e) => {
    ontouchend?.(e);
    longPress.ontouchend(e);
  }}
  ontouchcancel={(e) => {
    ontouchcancel?.(e);
    longPress.ontouchcancel(e);
  }}
  {...restProps}
/>
