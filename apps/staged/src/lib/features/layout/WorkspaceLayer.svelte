<script lang="ts">
  import { untrack, type Snippet } from 'svelte';
  import { navigation } from './navigation.svelte';
  import { createWorkspaceLayerState, setWorkspaceLayer } from './workspaceLayer';
  import { tabDirection } from '../../shared/tabDirection';
  import { firstTabbableAfter, lastTabbableBefore } from '../../shared/tabbable';

  interface Props {
    /** The base route shows this layer. */
    active: boolean;
    /**
     * Hide this layer with an inline clip-path while inactive. For the layer
     * on top, which would otherwise cover the active one (even while empty:
     * the host paints its own background); the bottom layer is simply
     * covered and never style-changed.
     */
    clipWhenInactive?: boolean;
    element?: HTMLDivElement | null;
    children: Snippet;
  }

  let { active, clipWhenInactive = false, element = $bindable(null), children }: Props = $props();

  // inactive → active flips; views inside key their per-visit work on it.
  // The initial value is deliberately the mount-time one: the first run of
  // the effect compares against it, so mounting active counts as zero flips.
  let activations = $state(0);
  // svelte-ignore state_referenced_locally
  let wasActive = active;
  $effect.pre(() => {
    if (active && !wasActive) untrack(() => activations++);
    wasActive = active;
  });

  setWorkspaceLayer(
    createWorkspaceLayerState({
      active: () => active,
      parked: () => navigation.workspaceParked,
      activations: () => activations,
    })
  );

  // The host of the layer the base route shows. Layers don't carry aria-hidden
  // for the parked state (the workspace above them does), so this resolves
  // under settings / diff too.
  function activeLayerHost(): HTMLElement | null {
    return document.querySelector<HTMLElement>('.workspace-layer:not([aria-hidden])');
  }

  // An inactive layer is covered or clipped, not inert: `inert` restyles the
  // whole subtree on WebKit (~55 ms on a 4k-node tree), and the views in here
  // still call `.focus()`. So it stays in the tab order and this guard sends
  // focus that lands in it on: a Tab carries on past the layer in its
  // direction, and focus that arrived by click or `.focus()` goes back to the
  // control it came from (or to the active layer's host when that control is
  // gone or was inside here). App.svelte's workspace guard owns the parked
  // case, where everything under the detail host is covered.
  function handleFocusIn(e: FocusEvent) {
    if (active || navigation.workspaceParked || !element) return;
    if (tabDirection.inFlight === 'forward') {
      (firstTabbableAfter(element) ?? activeLayerHost())?.focus({ preventScroll: true });
      return;
    }
    if (tabDirection.inFlight === 'backward') {
      (lastTabbableBefore(element) ?? activeLayerHost())?.focus({ preventScroll: true });
      return;
    }
    const from =
      e.relatedTarget instanceof HTMLElement && !element.contains(e.relatedTarget)
        ? e.relatedTarget
        : null;
    from?.focus({ preventScroll: true });
    // Connected is not focusable: `.focus()` on a hidden control is a silent
    // no-op, which would leave focus in here.
    if (document.activeElement !== from) activeLayerHost()?.focus({ preventScroll: true });
  }

  // Going inactive with focus inside: move it to the layer that took over.
  $effect(() => {
    if (active || !element) return;
    if (element.contains(document.activeElement)) activeLayerHost()?.focus({ preventScroll: true });
  });
</script>

<!-- Only aria-hidden and (for the top layer) the inline clip-path ever change
     on this root; any other style flip would restyle the whole subtree. -->
<div
  class="workspace-layer"
  tabindex="-1"
  aria-hidden={!active || undefined}
  style:clip-path={clipWhenInactive && !active ? 'inset(50%)' : undefined}
  bind:this={element}
  onfocusin={handleFocusIn}
>
  {@render children()}
</div>

<style>
  /* Both layers share the workspace's one grid cell; tree order puts the
     project layer on top. Opaque, so the covered layer never shows through,
     and its own stacking context so nothing in a lower layer can rise above. */
  .workspace-layer {
    grid-area: 1 / 1;
    display: flex;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    background-color: var(--bg-chrome);
    isolation: isolate;
    outline: none;
  }
</style>
