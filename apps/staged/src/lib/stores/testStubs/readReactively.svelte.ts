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

/**
 * Read `$derived(read())` from an effect, the way a template reads a
 * component's $derived: `read` re-runs whenever something it read changes,
 * but `onChange` runs only when that gives a different value. Call the
 * returned function to stop.
 */
export function readDerivedReactively<T>(read: () => T, onChange: (value: T) => void): () => void {
  return $effect.root(() => {
    const value = $derived.by(read);
    $effect.pre(() => {
      onChange(value);
    });
  });
}
