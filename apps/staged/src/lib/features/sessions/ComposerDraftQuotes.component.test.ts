import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import ComposerDraftQuotes from './ComposerDraftQuotes.svelte';
import type { DraftQuote } from './composerDraft';
import { reactiveProps } from './testStubs/reactiveProps.svelte';

// The quote list's keyboard and composition behaviour: Backspace in an empty
// gap removes the quote below it (or, once the quote has a reply, moves the
// caret into the reply rather than deleting it), the X button leaves focus
// where it was so the composer can move it on, and a composition open in any
// gap or reply is reported through one `composing` flag so the composer's
// Send gate sees it — the main input isn't the only place a word can be
// half-typed.

function quote(id: string, before = '', reply = ''): DraftQuote {
  return { id, before, quote: `Quoted ${id}`, headings: [], reply };
}

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;

function render(quotes: DraftQuote[]) {
  const onRemoveQuote = vi.fn();
  const oncompositionend = vi.fn();
  const props = reactiveProps({
    quotes,
    composing: false,
    onRemoveQuote,
    oncompositionend,
  });
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ComposerDraftQuotes, { target, props });
  flushSync();
  return { props, onRemoveQuote, oncompositionend };
}

function gapOf(id: string): HTMLElement {
  const gap = target.querySelector<HTMLElement>(`[data-quote-entry="${id}"] .composer-gap-input`);
  if (!gap) throw new Error(`no gap for ${id}`);
  return gap;
}

function replyOf(id: string): HTMLElement {
  const reply = target.querySelector<HTMLElement>(
    `[data-quote-entry="${id}"] .composer-quote-reply`
  );
  if (!reply) throw new Error(`no reply for ${id}`);
  return reply;
}

function removeButtonOf(id: string): HTMLElement {
  const button = target.querySelector<HTMLElement>(
    `[data-quote-entry="${id}"] .composer-quote-remove`
  );
  if (!button) throw new Error(`no remove button for ${id}`);
  return button;
}

function backspace(el: HTMLElement, init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent('keydown', {
    key: 'Backspace',
    bubbles: true,
    cancelable: true,
    ...init,
  });
  el.dispatchEvent(event);
  flushSync();
  return event;
}

beforeEach(() => {
  // jsdom lays nothing out and has no scrollIntoView; the list only needs it to exist.
  if (!('scrollIntoView' in Element.prototype)) {
    Object.defineProperty(Element.prototype, 'scrollIntoView', {
      configurable: true,
      value: () => {},
    });
  }
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  target.remove();
});

describe('ComposerDraftQuotes Backspace', () => {
  it('removes the quote below an empty gap, once per press', () => {
    const { onRemoveQuote } = render([quote('a'), quote('b')]);

    const event = backspace(gapOf('b'));

    expect(onRemoveQuote).toHaveBeenCalledWith('b');
    expect(event.defaultPrevented).toBe(true);

    backspace(gapOf('a'), { repeat: true });
    expect(onRemoveQuote).toHaveBeenCalledTimes(1);
  });

  it('keeps a quote with a reply, moving the caret to the end of the reply', () => {
    const { onRemoveQuote } = render([quote('a'), quote('b', '', 'typed reply')]);
    gapOf('b').focus();

    const event = backspace(gapOf('b'));

    expect(onRemoveQuote).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(replyOf('b'));
    const selection = window.getSelection();
    if (!selection?.anchorNode) throw new Error('no selection');
    expect(selection.isCollapsed).toBe(true);
    // Everything in the reply is before the caret; nothing is after it.
    const beforeCaret = document.createRange();
    beforeCaret.selectNodeContents(replyOf('b'));
    beforeCaret.setEnd(selection.anchorNode, selection.anchorOffset);
    expect(beforeCaret.toString()).toBe('typed reply');
    const afterCaret = document.createRange();
    afterCaret.selectNodeContents(replyOf('b'));
    afterCaret.setStart(selection.anchorNode, selection.anchorOffset);
    expect(afterCaret.toString()).toBe('');
  });

  it('deletes text as usual while the gap holds any', () => {
    const { onRemoveQuote } = render([quote('a', 'some text')]);

    const event = backspace(gapOf('a'));

    expect(onRemoveQuote).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it('leaves a modified Backspace and one inside a composition to the editor', () => {
    const { onRemoveQuote } = render([quote('a')]);

    backspace(gapOf('a'), { metaKey: true });
    backspace(gapOf('a'), { altKey: true });
    gapOf('a').dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    flushSync();
    backspace(gapOf('a'));

    expect(onRemoveQuote).not.toHaveBeenCalled();
  });
});

describe('ComposerDraftQuotes composition', () => {
  it('reports a composition open in a reply or a gap, and its end', () => {
    const { props, oncompositionend } = render([quote('a'), quote('b')]);

    replyOf('a').dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    flushSync();
    expect(props.composing).toBe(true);

    replyOf('a').dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }));
    flushSync();
    expect(props.composing).toBe(false);
    expect(oncompositionend).toHaveBeenCalledTimes(1);

    gapOf('b').dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    flushSync();
    expect(props.composing).toBe(true);
  });

  it('drops the flag when the composing quote is removed from the draft', () => {
    const { props } = render([quote('a'), quote('b')]);

    replyOf('b').dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    flushSync();
    expect(props.composing).toBe(true);

    props.quotes = [quote('a')];
    flushSync();

    expect(props.composing).toBe(false);
  });
});

describe('ComposerDraftQuotes remove button', () => {
  it('does not take focus on mousedown, so the composer can see where it was', () => {
    render([quote('a', '', 'typed reply')]);
    replyOf('a').focus();

    const event = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    removeButtonOf('a').dispatchEvent(event);
    flushSync();

    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(replyOf('a'));
  });
});

describe('ComposerDraftQuotes keyboard cap', () => {
  it('scrolls the input that takes focus into view', () => {
    render([quote('a'), quote('b')]);
    const reveal = vi.spyOn(replyOf('b'), 'scrollIntoView');

    replyOf('b').focus();
    flushSync();

    expect(reveal).toHaveBeenCalledWith({ block: 'nearest' });
  });
});

describe('ComposerDraftQuotes focus helpers', () => {
  it('puts the caret at the end of a quote’s reply, for a quote just added from the note', () => {
    render([quote('a', '', 'so far')]);
    const list = component as unknown as { focusReply(id: string): boolean };

    expect(list.focusReply('a')).toBe(true);
    expect(document.activeElement).toBe(replyOf('a'));
    const selection = window.getSelection();
    if (!selection?.anchorNode) throw new Error('no selection');
    expect(selection.isCollapsed).toBe(true);
    const beforeCaret = document.createRange();
    beforeCaret.selectNodeContents(replyOf('a'));
    beforeCaret.setEnd(selection.anchorNode, selection.anchorOffset);
    expect(beforeCaret.toString()).toBe('so far');

    expect(list.focusReply('missing')).toBe(false);
  });

  it('tells which quote holds focus and can focus a gap at either end', () => {
    render([quote('a', 'before a'), quote('b')]);
    const list = component as unknown as {
      holdsFocus(id: string): boolean;
      focusGap(id: string, caret: 'start' | 'end'): boolean;
    };

    replyOf('a').focus();
    expect(list.holdsFocus('a')).toBe(true);
    expect(list.holdsFocus('b')).toBe(false);

    expect(list.focusGap('a', 'end')).toBe(true);
    expect(document.activeElement).toBe(gapOf('a'));
    const selection = window.getSelection();
    expect(selection?.anchorNode && gapOf('a').contains(selection.anchorNode)).toBe(true);
    expect(selection?.toString()).toBe('');

    expect(list.focusGap('missing', 'start')).toBe(false);
  });
});
