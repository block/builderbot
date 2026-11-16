// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { focusIfDropped, focusThroughMove, neighbourAfterRemove } from './focusAfterRemove';

let layerHost: HTMLElement;
let cards: Map<string, HTMLButtonElement>;

/** A layer host holding one card per id, as the grid and the sidebar render. */
function render(ids: string[]) {
  layerHost = document.createElement('div');
  layerHost.className = 'workspace-layer';
  layerHost.tabIndex = -1;
  document.body.appendChild(layerHost);
  cards = new Map(
    ids.map((id) => {
      const card = document.createElement('button');
      card.textContent = id;
      layerHost.appendChild(card);
      return [id, card];
    })
  );
}

/** Take a card off screen, as the remove does to its own. */
function remove(id: string) {
  cards.get(id)!.remove();
}

beforeEach(() => {
  render(['a', 'b', 'c']);
});

afterEach(() => {
  document.body.innerHTML = '';
});

describe('neighbourAfterRemove', () => {
  it('picks the card that takes the removed one’s place', () => {
    const resolve = neighbourAfterRemove(['a', 'b', 'c'], 'b', (id) => cards.get(id));
    remove('b');
    expect(resolve(null)).toBe(cards.get('c'));
  });

  it('falls back to the closest earlier card when nothing follows', () => {
    const resolve = neighbourAfterRemove(['a', 'b', 'c'], 'c', (id) => cards.get(id));
    remove('c');
    expect(resolve(null)).toBe(cards.get('b'));
  });

  it('skips cards hidden or gone since the remove was picked', () => {
    const hidden = new Set(['c']);
    const resolve = neighbourAfterRemove(['a', 'b', 'c', 'd'], 'b', (id) =>
      hidden.has(id) ? null : cards.get(id)
    );
    remove('b');
    // d was never rendered, c is hidden: a is what's left.
    expect(resolve(null)).toBe(cards.get('a'));
  });

  it('prefers the row of the project opened instead, wherever it sits', () => {
    // A filtered sidebar showing [a, b, c], where removing the open b opened
    // e from the whole list. The sidebar appends e as the selected row.
    const opened = document.createElement('button');
    layerHost.appendChild(opened);
    const rows = new Map([...cards, ['e', opened]]);
    const resolve = neighbourAfterRemove(['a', 'b', 'c'], 'b', (id) => rows.get(id));
    remove('b');
    expect(resolve('e')).toBe(opened);
  });

  it('falls back to the neighbour when the project opened instead isn’t shown', () => {
    const resolve = neighbourAfterRemove(['a', 'b', 'c'], 'b', (id) => cards.get(id));
    remove('b');
    expect(resolve('e')).toBe(cards.get('c'));
  });

  it('goes to the active layer’s host when no card is left', () => {
    document.body.innerHTML = '';
    render(['a']);
    const covered = document.createElement('div');
    covered.className = 'workspace-layer';
    covered.setAttribute('aria-hidden', 'true');
    document.body.prepend(covered);

    const resolve = neighbourAfterRemove(['a'], 'a', (id) => cards.get(id));
    remove('a');
    expect(resolve(null)).toBe(layerHost);
  });
});

describe('focusIfDropped', () => {
  it('moves focus that dropped to <body>', () => {
    cards.get('b')!.focus();
    remove('b');
    expect(document.activeElement).toBe(document.body);

    focusIfDropped(() => cards.get('c')!);
    expect(document.activeElement).toBe(cards.get('c'));
  });

  it('leaves focus that is somewhere else', () => {
    cards.get('a')!.focus();

    focusIfDropped(() => cards.get('c')!);
    expect(document.activeElement).toBe(cards.get('a'));
  });
});

describe('focusThroughMove', () => {
  /** Re-insert a card elsewhere, as a keyed list does when its order changes. */
  function moveToEnd(id: string) {
    layerHost.appendChild(cards.get(id)!);
  }

  /** The render and the menu's focus return have both happened. */
  async function settled() {
    await Promise.resolve();
    await new Promise((resolve) => setTimeout(resolve, 0));
  }

  async function nextFrame() {
    await new Promise((resolve) => requestAnimationFrame(resolve));
  }

  const shown = (id: string) => (cards.get(id)?.isConnected ? cards.get(id) : null);

  it('puts focus back on a row that lost it by moving', async () => {
    const focus = focusThroughMove(shown);
    focus.moving(['a', 'b', 'c'], 'a');
    // The menu hands focus back to the row, then the list re-sorts.
    cards.get('a')!.focus();
    moveToEnd('a');
    expect(document.activeElement).toBe(document.body);

    focus.menuClosed();
    await settled();
    expect(document.activeElement).toBe(cards.get('a'));
    await nextFrame();
    expect(document.activeElement).toBe(cards.get('a'));
  });

  it('waits for the menu to close', async () => {
    const focus = focusThroughMove(shown);
    focus.moving(['a', 'b', 'c'], 'a');
    await settled();
    await nextFrame();
    expect(document.activeElement).toBe(document.body);
  });

  it('goes to the row that took its place when the move took it off the list', async () => {
    const focus = focusThroughMove(shown);
    focus.moving(['a', 'b', 'c'], 'b');
    remove('b');

    focus.menuClosed();
    await settled();
    expect(document.activeElement).toBe(cards.get('c'));
    await nextFrame();
    expect(document.activeElement).toBe(cards.get('c'));
  });

  it('leaves focus the menu gave back to something else', async () => {
    const focus = focusThroughMove(shown);
    focus.moving(['a', 'b', 'c'], 'a');
    moveToEnd('a');
    cards.get('c')!.focus();

    focus.menuClosed();
    await settled();
    await nextFrame();
    expect(document.activeElement).toBe(cards.get('c'));
  });

  it('catches focus that drops a frame after the menu closed', async () => {
    const focus = focusThroughMove(shown);
    focus.moving(['a', 'b', 'c'], 'a');
    cards.get('a')!.focus();

    focus.menuClosed();
    await settled();
    expect(document.activeElement).toBe(cards.get('a'));
    moveToEnd('a');
    expect(document.activeElement).toBe(document.body);

    await nextFrame();
    expect(document.activeElement).toBe(cards.get('a'));
  });

  it('does nothing when the menu closes without a pick', async () => {
    const focus = focusThroughMove(shown);

    focus.menuClosed();
    await settled();
    await nextFrame();
    expect(document.activeElement).toBe(document.body);
  });

  it('acts once per pick', async () => {
    const focus = focusThroughMove(shown);
    focus.moving(['a', 'b', 'c'], 'a');
    focus.menuClosed();
    await settled();
    await nextFrame();
    expect(document.activeElement).toBe(cards.get('a'));

    // The next menu closes with nothing picked: focus is not pulled back.
    (document.activeElement as HTMLElement).blur();
    focus.menuClosed();
    await settled();
    await nextFrame();
    expect(document.activeElement).toBe(document.body);
  });
});
