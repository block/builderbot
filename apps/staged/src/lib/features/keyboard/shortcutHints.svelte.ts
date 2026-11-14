/**
 * A shortcut's keys as text, for a control that stands in for the key (a
 * button's tooltip).
 *
 * Read from the registry rather than written out, so the text is the keys
 * the shortcut is bound to: the user's after a rebind in Keyboard settings,
 * and `Ctrl+K` where the registry maps the Command modifier to Ctrl. The
 * registry is plain module state, so this is what makes a read of it
 * reactive: a `$derived` or template that calls shortcutHint re-runs when a
 * shortcut is registered or withdrawn or a binding changes.
 */

import { createSubscriber } from 'svelte/reactivity';
import { formatShortcutLabel, getShortcutBinding, subscribeToShortcutChanges } from './shortcuts';

const subscribe = createSubscriber((update) => subscribeToShortcutChanges(update));

/** The keys for the shortcut registered under `id`, or null when there is
 *  none (App.svelte registers them once the saved bindings have loaded). */
export function shortcutHint(id: string): string | null {
  subscribe();
  const binding = getShortcutBinding(id);
  return binding ? formatShortcutLabel(binding) : null;
}
