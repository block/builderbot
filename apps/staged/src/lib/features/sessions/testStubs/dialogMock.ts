/**
 * Factory for `vi.mock('$lib/components/ui/dialog', dialogModuleMock)` in
 * component tests.
 *
 * Root/Content/Title pass their children through so the dialog body renders;
 * every other PascalCase export (ResizeHandle, Overlay, whatever lands on main
 * next) resolves to the empty stub. A closed mock object would instead fail
 * every suite that renders a dialog with an opaque "not a component" error
 * the moment a new `Dialog.*` subcomponent is used upstream.
 */
export async function dialogModuleMock(): Promise<Record<string, unknown>> {
  const { default: Passthrough } = await import('./PassthroughStub.svelte');
  const { default: Empty } = await import('./EmptyStub.svelte');
  const explicit: Record<string, unknown> = {
    Root: Passthrough,
    Content: Passthrough,
    Title: Passthrough,
  };
  // Only component-shaped names fall back to the stub: `then`, `default` and
  // `__esModule` must stay absent or module interop misreads the mock.
  const isComponentName = (prop: PropertyKey): prop is string =>
    typeof prop === 'string' && /^[A-Z]/.test(prop);
  return new Proxy(explicit, {
    has: (target, prop) => prop in target || isComponentName(prop),
    get: (target, prop) => {
      if (prop in target) return target[prop as string];
      return isComponentName(prop) ? Empty : undefined;
    },
  });
}
