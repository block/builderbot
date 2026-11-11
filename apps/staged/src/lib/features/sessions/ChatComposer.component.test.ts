import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import ChatComposer from './ChatComposer.svelte';
import type { DraftQuote } from './composerDraft';
import { composerSendControl } from './composerSendControl';
import { reactiveProps } from './testStubs/reactiveProps.svelte';

// The composer's field: one box holding the quote blocks, the text between
// them and the main input, which the user works as one editor. Covered here
// are the ways the field ties its inputs together — the arrows walking from
// one into the next, a click on its surface landing the caret in an input,
// the single placeholder, the keyboard cap on the whole field and the
// focused input kept in view — while the inputs' own behaviour is under
// ComposerDraftQuotes.component.test.ts.

// The on-screen-keyboard cap reads the visible viewport; stand it in with a
// keyboard up, so the cap is a share of the 400px left.
vi.mock('../../shared/visualViewport.svelte', () => ({
  visibleViewport: { height: 400 },
  watchVisualViewport: () => () => {},
}));

function quote(id: string, before = '', reply = ''): DraftQuote {
  return { id, before, quote: `Quoted ${id}`, headings: [], reply };
}

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;

function render(quotes: DraftQuote[], value = '') {
  const props = reactiveProps({
    value,
    quotes,
    // The control the pane would hand an idle session's composer; its
    // placeholder is the field's one.
    sendControl: composerSendControl({
      isLive: false,
      canSendLive: false,
      mode: 'queue',
      canStartNewSession: false,
    }),
    onRemoveQuote: vi.fn(),
  });
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ChatComposer, { target, props });
  flushSync();
  return props;
}

function field(): HTMLElement {
  const el = target.querySelector<HTMLElement>('.composer-field');
  if (!el) throw new Error('no field');
  return el;
}

function editors(): HTMLElement[] {
  return [...field().querySelectorAll<HTMLElement>('.hashtag-editor')];
}

function mainInput(): HTMLElement {
  const el = target.querySelector<HTMLElement>('.message-input');
  if (!el) throw new Error('no main input');
  return el;
}

function replyOf(id: string): HTMLElement {
  const el = target.querySelector<HTMLElement>(`[data-quote-entry="${id}"] .composer-quote-reply`);
  if (!el) throw new Error(`no reply for ${id}`);
  return el;
}

function gapOf(id: string): HTMLElement {
  const el = target.querySelector<HTMLElement>(`[data-quote-entry="${id}"] .composer-gap-input`);
  if (!el) throw new Error(`no gap for ${id}`);
  return el;
}

function focusWithCaret(el: HTMLElement) {
  el.focus();
  const range = document.createRange();
  range.selectNodeContents(el);
  range.collapse(true);
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function arrow(el: HTMLElement, key: 'ArrowUp' | 'ArrowDown', init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init });
  el.dispatchEvent(event);
  flushSync();
  return event;
}

beforeEach(() => {
  // jsdom lays nothing out and has no scrollIntoView; the field only needs it to exist.
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
  window.getSelection()?.removeAllRanges();
});

describe('ChatComposer field', () => {
  it('holds the quote list and the main input in one box, in document order', () => {
    render([quote('a'), quote('b')]);

    expect(field().querySelector('.composer-draft-quotes')).not.toBeNull();
    expect(field().contains(mainInput())).toBe(true);
    expect(editors()).toEqual([gapOf('a'), replyOf('a'), gapOf('b'), replyOf('b'), mainInput()]);
    expect(field().classList.contains('has-quotes')).toBe(true);
  });

  it('caps the whole field to a share of the visible viewport while the keyboard is up', () => {
    render([quote('a')]);

    expect(field().style.getPropertyValue('--composer-field-cap')).toBe('160px');
  });

  it('scrolls the input that takes focus into view', () => {
    render([quote('a'), quote('b')]);
    const reveal = vi.spyOn(replyOf('b'), 'scrollIntoView');

    replyOf('b').focus();
    flushSync();

    expect(reveal).toHaveBeenCalledWith({ block: 'nearest' });
  });
});

describe('ChatComposer placeholder', () => {
  it('marks the field once anything is typed anywhere, so only an empty draft shows one', () => {
    const props = render([quote('a')]);
    expect(field().classList.contains('has-draft-text')).toBe(false);

    props.quotes = [quote('a', '', 'a reply')];
    flushSync();
    expect(field().classList.contains('has-draft-text')).toBe(true);

    props.quotes = [quote('a', 'lead-in')];
    flushSync();
    expect(field().classList.contains('has-draft-text')).toBe(true);

    props.quotes = [quote('a')];
    props.value = 'trailing';
    flushSync();
    expect(field().classList.contains('has-draft-text')).toBe(true);
  });
});

describe('ChatComposer surface click', () => {
  it('puts the caret in the nearest input — the last, when nothing is nearer', () => {
    render([quote('a')]);

    field().dispatchEvent(new MouseEvent('click', { bubbles: true, clientY: 300 }));
    flushSync();

    expect(document.activeElement).toBe(mainInput());
  });

  it('leaves a click on an input or a block to them', () => {
    render([quote('a')]);
    gapOf('a').focus();

    gapOf('a').dispatchEvent(new MouseEvent('click', { bubbles: true, clientY: 10 }));
    flushSync();
    expect(document.activeElement).toBe(gapOf('a'));

    const block = target.querySelector<HTMLElement>('.composer-quote');
    if (!block) throw new Error('no block');
    block.dispatchEvent(new MouseEvent('click', { bubbles: true, clientY: 10 }));
    flushSync();
    // The block's own padding takes the caret to its reply, not the field's nearest.
    expect(document.activeElement).toBe(replyOf('a'));
  });
});

describe('ChatComposer arrows between inputs', () => {
  it('moves down from the last line of a reply into the main input, and up again', () => {
    render([quote('a')]);
    focusWithCaret(replyOf('a'));

    const down = arrow(replyOf('a'), 'ArrowDown');
    expect(down.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(mainInput());

    const up = arrow(mainInput(), 'ArrowUp');
    expect(up.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(replyOf('a'));
  });

  it('stops at the field’s ends', () => {
    render([quote('a')]);
    focusWithCaret(gapOf('a'));

    expect(arrow(gapOf('a'), 'ArrowUp').defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(gapOf('a'));

    focusWithCaret(mainInput());
    expect(arrow(mainInput(), 'ArrowDown').defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(mainInput());
  });

  it('leaves a modified arrow, a composing one and one already handled alone', () => {
    render([quote('a')]);
    focusWithCaret(replyOf('a'));

    expect(arrow(replyOf('a'), 'ArrowDown', { shiftKey: true }).defaultPrevented).toBe(false);
    expect(arrow(replyOf('a'), 'ArrowDown', { metaKey: true }).defaultPrevented).toBe(false);
    expect(arrow(replyOf('a'), 'ArrowDown', { isComposing: true }).defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(replyOf('a'));

    const handled = new KeyboardEvent('keydown', {
      key: 'ArrowDown',
      bubbles: true,
      cancelable: true,
    });
    replyOf('a').addEventListener('keydown', (e) => e.preventDefault(), { once: true });
    replyOf('a').dispatchEvent(handled);
    flushSync();
    expect(document.activeElement).toBe(replyOf('a'));
  });

  it('leaves the arrow to an input whose caret line it can’t tell', () => {
    render([quote('a', '', 'two\nlines')]);
    focusWithCaret(replyOf('a'));

    // jsdom gives the caret no rect, so the line is unknown and the key is left alone.
    expect(arrow(replyOf('a'), 'ArrowDown').defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(replyOf('a'));
  });
});
