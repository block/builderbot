// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { focusIfDropped, neighbourAfterRemove } from './focusAfterRemove';

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
    expect(resolve()).toBe(cards.get('c'));
  });

  it('falls back to the closest earlier card when nothing follows', () => {
    const resolve = neighbourAfterRemove(['a', 'b', 'c'], 'c', (id) => cards.get(id));
    remove('c');
    expect(resolve()).toBe(cards.get('b'));
  });

  it('skips cards hidden or gone since the remove was picked', () => {
    const hidden = new Set(['c']);
    const resolve = neighbourAfterRemove(['a', 'b', 'c', 'd'], 'b', (id) =>
      hidden.has(id) ? null : cards.get(id)
    );
    remove('b');
    // d was never rendered, c is hidden: a is what's left.
    expect(resolve()).toBe(cards.get('a'));
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
    expect(resolve()).toBe(layerHost);
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
