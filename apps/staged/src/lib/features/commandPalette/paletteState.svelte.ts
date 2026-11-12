/**
 * Open state for the ⌘K project palette (CommandPalette.svelte), and the
 * shortcut that toggles it.
 *
 * Module-scoped so the global shortcut registered in App.svelte and the
 * dialog mounted beside the app's other shared dialogs agree without either
 * owning the other.
 */

import type { Shortcut } from '../keyboard/shortcuts';

/** Marks the palette's own dialog (CommandPalette.svelte sets it), so it
 *  doesn't count as "another modal". */
export const COMMAND_PALETTE_ATTRIBUTE = 'data-command-palette';

export const commandPalette = $state({
  open: false,
  /** The search field's text. Cleared on every open. */
  query: '',
});

/**
 * Whether a modal other than the palette is open: a dialog or alert dialog
 * (design-system or hand-rolled, they all carry the role), not counting one
 * that is only playing its exit animation.
 *
 * The palette stays shut while one is up. Opening over a modal would be
 * destructive in ways the palette can't see from here: a pending "Remove
 * project?" would outlive the project it was asked about, a form's
 * window-level Enter handler would submit on the Enter that picks a result,
 * and navigating unmounts the view that owns the modal along with whatever
 * was typed into it.
 *
 * This is the dialog half of the rule ⌘↑/⌘↓ follow (navigation's
 * isModalOpen), and only that half. Their rule also counts the diff view;
 * the palette opens over the diff view and over Settings on purpose, since
 * finding a project from anywhere is what it is for. Opening a result from
 * there replaces the workspace route underneath (selectProject), after the
 * diff view has saved what was typed into it (diffLeaveCheck).
 */
export function otherModalOpen(root: ParentNode = document): boolean {
  for (const modal of root.querySelectorAll('[role="dialog"], [role="alertdialog"]')) {
    if (modal.hasAttribute(COMMAND_PALETTE_ATTRIBUTE)) continue;
    if (modal.getAttribute('data-state') === 'closed') continue;
    return true;
  }
  return false;
}

/** Open the palette with an empty query. False, having done nothing, when
 *  another modal is in the way. */
export function openCommandPalette(): boolean {
  if (commandPalette.open) return true;
  if (otherModalOpen()) return false;
  commandPalette.query = '';
  commandPalette.open = true;
  return true;
}

export function closeCommandPalette(): void {
  commandPalette.open = false;
}

/** Open the palette, or close it when it is open. Does nothing while another
 *  modal is in the way. */
export function toggleCommandPalette(): void {
  if (commandPalette.open) closeCommandPalette();
  else openCommandPalette();
}

/**
 * The ⌘K shortcut (Ctrl+K off macOS), for the app's shortcut registry:
 * App.svelte registers it with the rest, which also lists it in Keyboard
 * settings. It works from text fields, the palette's own included, so the
 * same key closes it.
 *
 * `enabled` says whether there is a workspace to search; App.svelte passes
 * false for its store-reset and error screens.
 *
 * The key is the palette's whether or not a press does anything. The handler
 * never returns the `false` that would make the registry pass the event on,
 * so a press that another dialog blocks, or one while disabled, is still
 * swallowed: ⌘K doesn't reach the webview's or the browser's own binding in
 * some states and not others. Auto-repeat is ignored (and swallowed too), so
 * holding the key opens the palette once instead of flickering it.
 */
export function commandPaletteShortcut(enabled: () => boolean = () => true): Shortcut {
  return {
    id: 'search-projects',
    description: 'Search projects',
    category: 'search',
    keys: ['k'],
    modifiers: { meta: true },
    allowInInputs: true,
    ignoreRepeat: true,
    handler: () => {
      if (enabled()) toggleCommandPalette();
    },
  };
}
