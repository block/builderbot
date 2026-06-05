/**
 * Svelte action that relocates an element to `document.body`.
 *
 * A `position: fixed` element is positioned relative to its nearest ancestor
 * that establishes a containing block — which any ancestor with a `transform`
 * does (e.g. shadcn's `Dialog.Content`, centred with `-translate-*`). Moving the
 * node to `<body>` removes it from such ancestors so its viewport-relative
 * coordinates are honoured. Also escapes any `overflow: hidden/auto` clipping.
 */
export function portal(node: HTMLElement) {
  document.body.appendChild(node);
  return {
    destroy() {
      node.remove();
    },
  };
}
