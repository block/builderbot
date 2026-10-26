/**
 * Svelte action that relocates an element to `#portal-root` (falling back to
 * `document.body`).
 *
 * A `position: fixed` element is positioned relative to its nearest ancestor
 * that establishes a containing block — which any ancestor with a `transform`
 * does (e.g. shadcn's `Dialog.Content`, centred with `-translate-*`). Moving the
 * node to `<body>` removes it from such ancestors so its viewport-relative
 * coordinates are honoured. Also escapes any `overflow: hidden/auto` clipping.
 * `#portal-root` is used rather than `<body>` itself for the same reason
 * bits-ui portals target it (see App.svelte): appending a new last child to
 * `<body>` changes `#app`'s `:last-child` state and restyles the whole app.
 */
export function portal(node: HTMLElement) {
  (document.getElementById('portal-root') ?? document.body).appendChild(node);
  return {
    destroy() {
      node.remove();
    },
  };
}
