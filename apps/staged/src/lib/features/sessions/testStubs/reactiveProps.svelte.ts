/**
 * Wrap a props object in $state so component tests can mutate props after
 * mount() and have the change propagate (plain objects passed to mount() are
 * read once and never update).
 */
export function reactiveProps<T extends Record<string, unknown>>(initial: T): T {
  const props = $state(initial);
  return props;
}
