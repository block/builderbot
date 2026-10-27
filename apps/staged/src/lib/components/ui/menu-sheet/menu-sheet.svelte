<!--
  MenuSheet — the mobile presentation shared by DropdownMenu.Content and
  ContextMenu.Content (see viewport.sheetMenus): an iOS action-sheet style
  panel that slides up from the bottom behind a scrim, with full-width rows
  sized for fingers and a Cancel row underneath.

  Used through the bits-ui `child` snippet of a *static* content primitive:
  `props` are the menu's own attributes (role, id, data-state, key handlers)
  and must land on the element that wraps every item; `cancel` is rendered by
  the caller as a real menu item so it closes through bits-ui.

  The scrim is the sheet's own hit target. While a menu is open bits-ui sets
  `pointer-events: none` on `body` and `auto` on the content (and on every
  context-menu trigger), so a scrim that merely inherited would be invisible
  to the tap, which would land on whatever sits underneath — a project card —
  and activate it as the menu closed. Opted back in, it takes the whole
  gesture and closes the menu through `onDismiss` on the trailing `click`, so
  a slow tap closes nothing until the finger lifts and nothing is left to land
  on the page; the sheet (scrim included) stays mounted through its exit
  animation. The scrim sits outside the content element, so bits-ui's own
  outside-interaction dismissal fires for the same tap too and agrees — but it
  cannot be relied on alone: it waits on a debounce timer that Chrome holds
  back while a touch is in progress, and for a quick tap the click can land
  before bits-ui is listening.

  Sub-menus (SubContentStatic) reuse the frame as a second sheet stacked over
  the parent's, with a Back row in the `cancel` slot that closes only the sub.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { ClassValue } from 'clsx';
  import { cn } from '$lib/components/utils.js';

  let {
    props,
    class: className,
    children,
    cancel,
    onDismiss,
  }: {
    props: Record<string, unknown>;
    class?: ClassValue | null;
    children?: Snippet;
    cancel?: Snippet;
    /** Closes this sheet's menu; a tap on the scrim lands here. */
    onDismiss?: () => void;
  } = $props();

  let state = $derived(props['data-state']);
</script>

<div
  class="menu-sheet fixed inset-0 z-(--z-index-floating) flex flex-col justify-end"
  data-state={state}
>
  <!-- Decorative: the Cancel/Back row is the accessible dismissal. -->
  <div
    class="menu-sheet-scrim pointer-events-auto data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0 absolute inset-0 bg-black/40 duration-200"
    data-state={state}
    aria-hidden="true"
    onclick={onDismiss}
  ></div>
  <div
    {...props}
    data-menu-sheet=""
    class={cn(
      'menu-sheet-panel data-open:animate-in data-open:slide-in-from-bottom data-closed:animate-out data-closed:slide-out-to-bottom text-popover-foreground relative flex max-h-[calc(100dvh-24px)] flex-col px-2 pb-[max(env(safe-area-inset-bottom),8px)] text-base outline-none duration-200',
      props.class as string | undefined,
      className
    )}
  >
    <div
      class="menu-sheet-group bg-popover ring-foreground/10 min-h-0 overflow-y-auto overflow-x-hidden rounded-2xl ring-1"
    >
      {@render children?.()}
    </div>
    {#if cancel}
      <div class="menu-sheet-group bg-popover ring-foreground/10 mt-2 shrink-0 rounded-2xl ring-1">
        {@render cancel()}
      </div>
    {/if}
  </div>
</div>

<style>
  /* Rows: finger-sized (52px), larger type, flush edges; hairlines between
     neighbours stand in for the popover's rounded-row look. Unlayered rules,
     so they win over the items' Tailwind utilities without specificity games. */
  :global([data-menu-sheet] [data-slot$='-item']),
  :global([data-menu-sheet] [data-slot$='-sub-trigger']) {
    min-height: 52px;
    padding: 12px 16px;
    gap: 12px;
    font-size: 16px;
    line-height: 1.3;
    border-radius: 0;
  }

  :global(
    [data-menu-sheet]
      :is([data-slot$='-item'], [data-slot$='-sub-trigger'])
      + :is([data-slot$='-item'], [data-slot$='-sub-trigger'])
  ) {
    border-top: 1px solid var(--border);
  }

  :global(
    [data-menu-sheet]
      :is([data-slot$='-item'], [data-slot$='-sub-trigger'])
      svg:not([class*='size-'])
  ) {
    width: 20px;
    height: 20px;
  }

  /* Radio/checkbox indicators sit at the right edge; give them room and
     keep them clear of the wider row padding. Inset rows align with icons. */
  :global([data-menu-sheet] [data-slot$='-radio-item']),
  :global([data-menu-sheet] [data-slot$='-checkbox-item']) {
    padding-right: 44px;
  }

  :global([data-menu-sheet] [data-slot$='-item-indicator']),
  :global(
    [data-menu-sheet] :is([data-slot$='-radio-item'], [data-slot$='-checkbox-item']) > span.absolute
  ) {
    right: 16px;
  }

  :global([data-menu-sheet] :is([data-slot$='-item'], [data-slot$='-sub-trigger'])[data-inset]) {
    padding-left: 48px;
  }

  /* Separators become grouped gaps instead of inset hairlines. */
  :global([data-menu-sheet] [data-slot$='-separator']) {
    height: 6px;
    margin: 0;
    background: var(--muted);
  }

  :global([data-menu-sheet] [data-slot$='-label']),
  :global([data-menu-sheet] [data-slot$='-group-heading']) {
    padding: 10px 16px 4px;
    font-size: 13px;
  }

  :global(.menu-sheet-cancel) {
    justify-content: center;
    font-weight: 600;
  }
</style>
