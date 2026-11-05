import { activeLayerHost } from '../layout/workspaceLayer';

/**
 * Where focus goes once a removed project's card or sidebar row is gone.
 *
 * A confirmed remove hides the project from every list at once, which
 * unmounts the card or row whose context menu it was picked from. That is
 * the element the menu and the confirmation dialog would hand focus back
 * to, so without this focus drops to `<body>`. The list the remove came from
 * supplies it, and projectActions resolves it once the project is hidden.
 *
 * `openedInstead` is the project the delete opened in the removed one's
 * place, when the removed project was the one open, else null.
 */
export type FocusAfterRemove = (openedInstead: string | null) => HTMLElement | null;

/**
 * The card or row that takes the removed one's place: the project opened
 * instead, if shown, as that is the row a list highlights; else the next
 * one shown, else the closest one before it, else the active layer's host.
 * The opened project needn't be a neighbour: the delete picks it from the
 * whole project list, and a filtered sidebar keeps the open project's row
 * even when the filter rules it out. `shownIds` is the list in screen order
 * as it stood when the remove was picked, since the removed project has
 * left it by the time this resolves. `elementFor` answers for what is on
 * screen now: null for a project hidden or gone since.
 */
export function neighbourAfterRemove(
  shownIds: readonly string[],
  removedId: string,
  elementFor: (projectId: string) => HTMLElement | null | undefined
): FocusAfterRemove {
  const index = shownIds.indexOf(removedId);
  const neighbours =
    index < 0 ? [] : [...shownIds.slice(index + 1), ...shownIds.slice(0, index).reverse()];
  return (openedInstead) => {
    const candidates = openedInstead === null ? neighbours : [openedInstead, ...neighbours];
    for (const projectId of candidates) {
      const element = elementFor(projectId);
      if (element?.isConnected) return element;
    }
    return activeLayerHost();
  };
}

/**
 * Focus what `resolve` names, but only if focus has dropped to `<body>`:
 * focus the user has since put somewhere else stays where it is.
 */
export function focusIfDropped(resolve: () => HTMLElement | null): void {
  const active = document.activeElement;
  if (active && active !== document.body) return;
  resolve()?.focus({ preventScroll: true });
}
