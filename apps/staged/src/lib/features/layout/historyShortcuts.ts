/**
 * The Back and Forward shortcuts, for the app's shortcut registry: App.svelte
 * registers them with the rest, which also lists them in Keyboard settings.
 *
 * They step through the views visited, like the top bar's buttons
 * (TopBarNavControls) and through the same two functions. ⌘[ and ⌘] are the
 * browser pair and the ones the buttons' tooltips name; ⌘← and ⌘→ do the
 * same. These are the only Back and Forward keys in the app: the diff view
 * has none of its own, so a rebound key works there too, and Back from a
 * diff waits for the comment being typed to save as any step does
 * (navigation's stepHistory).
 *
 * The keys are the app's whether or not a press does anything. The handlers
 * never return the `false` that would make the registry pass the event on,
 * so a press with no entry that way is still swallowed: left to the webview
 * the keys are its own page history, which in a browser tab would leave the
 * app. Text fields keep them (the registry skips shortcuts there by
 * default), where ⌘← and ⌘→ move the caret to the ends of the line.
 */

import type { Shortcut } from '../keyboard/shortcuts';
import { goBack, goForward } from './navigation.svelte';

/** The ids the shortcuts are registered under. */
export const GO_BACK_SHORTCUT_ID = 'app-go-back';
export const GO_FORWARD_SHORTCUT_ID = 'app-go-forward';

export function historyShortcuts(): Shortcut[] {
  return [
    {
      id: GO_BACK_SHORTCUT_ID,
      description: 'Back',
      category: 'app',
      keys: ['[', 'ArrowLeft'],
      modifiers: { meta: true },
      handler: () => {
        void goBack();
      },
    },
    {
      id: GO_FORWARD_SHORTCUT_ID,
      description: 'Forward',
      category: 'app',
      keys: [']', 'ArrowRight'],
      modifiers: { meta: true },
      handler: () => {
        void goForward();
      },
    },
  ];
}
