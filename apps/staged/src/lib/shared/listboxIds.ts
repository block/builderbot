/**
 * Ids for a text field whose popup is a listbox managed with
 * `aria-activedescendant` (RepoSearchInput's combobox, HashtagInput's textbox
 * with its # dropdown). The options take no focus; the input stays focused
 * and points assistive tech at the highlighted option by id, so every option
 * needs a stable id and the input needs to drop the reference whenever the
 * popup is closed or nothing is highlighted.
 */

/** Id of the option at `index` (its position in the listbox, from 0). */
export function listboxOptionId(listboxId: string, index: number): string {
  return `${listboxId}-option-${index}`;
}

export interface ActiveDescendantState {
  /** Whether the listbox is rendered. */
  open: boolean;
  /** Number of options in the listbox. */
  count: number;
  /** Highlighted option's index; -1 (or any index outside the list) for none. */
  selectedIndex: number;
}

/**
 * The `aria-activedescendant` value for the input: the highlighted option's
 * id while the listbox is open and the index is within it, otherwise
 * `undefined` so the attribute is removed.
 */
export function activeDescendantId(
  listboxId: string,
  { open, count, selectedIndex }: ActiveDescendantState
): string | undefined {
  if (!open) return undefined;
  if (!Number.isInteger(selectedIndex) || selectedIndex < 0 || selectedIndex >= count) {
    return undefined;
  }
  return listboxOptionId(listboxId, selectedIndex);
}
