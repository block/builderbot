/**
 * Open state for the ⌘K project palette (CommandPalette.svelte).
 *
 * Module-scoped so the global shortcut registered in App.svelte and the
 * dialog mounted beside the app's other shared dialogs agree without either
 * owning the other.
 */

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
 * The palette switches projects, and so follows the rule ⌘↑/⌘↓ already do
 * (navigation's isModalOpen): it stays shut while a modal is up. Opening over
 * one would be destructive in ways the palette can't see from here — a
 * pending "Remove project?" would outlive the project it was asked about,
 * a form's window-level Enter handler would submit on the Enter that picks a
 * result, and navigating unmounts the view that owns the modal along with
 * whatever was typed into it.
 */
export function otherModalOpen(root: ParentNode = document): boolean {
  for (const modal of root.querySelectorAll('[role="dialog"], [role="alertdialog"]')) {
    if (modal.hasAttribute(COMMAND_PALETTE_ATTRIBUTE)) continue;
    if (modal.getAttribute('data-state') === 'closed') continue;
    return true;
  }
  return false;
}

/** Open the palette with an empty query. False when a modal is in the way. */
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

/**
 * The ⌘K handler: open, or close when already open. Returns false when the
 * press did nothing, so the shortcut registry leaves the key event alone.
 */
export function toggleCommandPalette(): boolean {
  if (commandPalette.open) {
    closeCommandPalette();
    return true;
  }
  return openCommandPalette();
}
