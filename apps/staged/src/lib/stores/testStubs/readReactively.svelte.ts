/**
 * Read `read()` from an effect, the way a template or a component's $derived
 * does, so a store's $derived fields are connected and cached as the app sees
 * them. Needs the client build (the component project's jsdom). Call the
 * returned function to stop.
 */
export function readReactively(read: () => unknown): () => void {
  return $effect.root(() => {
    $effect.pre(() => {
      read();
    });
  });
}
