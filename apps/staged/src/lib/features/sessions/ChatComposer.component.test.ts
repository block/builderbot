import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import ChatComposer from './ChatComposer.svelte';
import type { ComposerDraft, DraftQuote } from './composerDraft';
import { composerSendControl } from './composerSendControl';
import ComposerDraftHost from './testStubs/ComposerDraftHost.svelte';
import { reactiveProps } from './testStubs/reactiveProps.svelte';
import {
  installNativeUndoStack,
  type NativeUndoStack,
} from '../../shared/testStubs/nativeUndoStack';

// The composer's field: one box holding the quote blocks, the text between
// them and the main input, which the user works as one editor. Covered here
// are the ways the field ties its inputs together — the arrows walking from
// one into the next, a press on its surface landing the caret in an input,
// the single placeholder, the keyboard cap on the whole field, the focused
// input kept in view by the field's own scroll, and a quote's removal taking
// its place in the browser's undo history with the typing around it — while
// the inputs' own behaviour is under ComposerDraftQuotes.component.test.ts.

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

function press(el: HTMLElement, init: MouseEventInit = {}) {
  const event = new MouseEvent('mousedown', { bubbles: true, cancelable: true, ...init });
  el.dispatchEvent(event);
  flushSync();
  return event;
}

function rect(top: number, bottom: number): DOMRect {
  return { top, bottom, height: bottom - top, left: 0, right: 10, width: 10 } as DOMRect;
}

/** Give the field a 100px view from 201 to 301 (1px border) and a scroll offset of its own. */
function layOutField(): HTMLElement {
  const el = field();
  Object.defineProperty(el, 'clientHeight', { value: 100 });
  Object.defineProperty(el, 'clientTop', { value: 1 });
  let scroll = 0;
  Object.defineProperty(el, 'scrollTop', {
    get: () => scroll,
    set: (value: number) => {
      scroll = value;
    },
  });
  el.getBoundingClientRect = () => rect(200, 302);
  return el;
}

beforeEach(() => {
  // jsdom has no scrollIntoView; the tests watch that the field doesn't reach for it.
  if (!('scrollIntoView' in Element.prototype)) {
    Object.defineProperty(Element.prototype, 'scrollIntoView', {
      configurable: true,
      writable: true,
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

  it('scrolls the field, and nothing above it, to the input that takes focus', () => {
    render([quote('a'), quote('b')]);
    const fieldEl = layOutField();
    replyOf('b').getBoundingClientRect = () => rect(320, 350);
    const ancestors = vi.spyOn(Element.prototype, 'scrollIntoView');

    replyOf('b').focus();
    flushSync();

    expect(fieldEl.scrollTop).toBe(49);
    expect(ancestors).not.toHaveBeenCalled();
    ancestors.mockRestore();
  });

  it('brings a caret it places in a tall input into view, once the caret is in, and never scrolls the page', () => {
    render([quote('a')], 'abc');
    const fieldEl = layOutField();
    // The main input is taller than the field's view, so only the caret's
    // line can say where to scroll — and at `focusin` the caret isn't placed
    // yet. Its first and last lines are the same, just under the view.
    mainInput().getBoundingClientRect = () => rect(320, 720);
    const originalRects = Object.getOwnPropertyDescriptor(Range.prototype, 'getClientRects');
    Object.defineProperty(Range.prototype, 'getClientRects', {
      configurable: true,
      writable: true,
      value(this: Range) {
        return this.collapsed ? [] : [rect(320, 340)];
      },
    });
    const focus = vi.spyOn(HTMLElement.prototype, 'focus');
    const ancestors = vi.spyOn(Element.prototype, 'scrollIntoView');
    try {
      focusWithCaret(replyOf('a'));
      // The reply has no box in jsdom; the focus above scrolled to its 0.
      fieldEl.scrollTop = 0;
      focus.mockClear();

      arrow(replyOf('a'), 'ArrowDown');
      expect(document.activeElement).toBe(mainInput());
      expect(fieldEl.scrollTop).toBe(39);
      // The browser's own scroll-on-focus is turned off: it would take the
      // dialog and the page along.
      expect(focus).toHaveBeenCalledWith({ preventScroll: true });

      fieldEl.scrollTop = 0;
      press(fieldEl, { clientY: 800 });
      expect(document.activeElement).toBe(mainInput());
      expect(fieldEl.scrollTop).toBe(39);
      expect(ancestors).not.toHaveBeenCalled();
    } finally {
      focus.mockRestore();
      ancestors.mockRestore();
      if (originalRects) Object.defineProperty(Range.prototype, 'getClientRects', originalRects);
      else delete (Range.prototype as Partial<Range>).getClientRects;
    }
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

describe('ChatComposer surface press', () => {
  it('puts the caret in the nearest input on mousedown — the last, when nothing is nearer', () => {
    render([quote('a')]);
    gapOf('a').focus();

    const event = press(field(), { clientY: 300 });

    expect(document.activeElement).toBe(mainInput());
    // The press's default is prevented, so the gap was never blurred to the page.
    expect(event.defaultPrevented).toBe(true);
  });

  it('leaves a press on an input or a block to them', () => {
    render([quote('a')]);
    gapOf('a').focus();

    expect(press(gapOf('a'), { clientY: 10 }).defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(gapOf('a'));

    const block = target.querySelector<HTMLElement>('.composer-quote');
    if (!block) throw new Error('no block');
    press(block, { clientY: 10 });
    // The block's own padding takes the caret to its reply, not the field's nearest.
    expect(document.activeElement).toBe(replyOf('a'));
  });

  it('leaves a Shift-press, a secondary button and a click alone', () => {
    render([quote('a')]);
    gapOf('a').focus();

    expect(press(field(), { clientY: 300, shiftKey: true }).defaultPrevented).toBe(false);
    expect(press(field(), { clientY: 300, button: 2 }).defaultPrevented).toBe(false);
    field().dispatchEvent(new MouseEvent('click', { bubbles: true, clientY: 300 }));
    flushSync();
    expect(document.activeElement).toBe(gapOf('a'));
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

// Removing a quote is a step on the browser's undo stack, in order with the
// typing in the field's inputs. jsdom has no such stack; the stand-in walks
// edits the way a browser does (testStubs/nativeUndoStack.ts), and the host
// keeps the draft as the pane would.
describe('ChatComposer quote removal', () => {
  let stack: NativeUndoStack;
  let host: ReturnType<typeof mountHost>;

  function mountHost() {
    return mount(ComposerDraftHost, { target });
  }

  function renderDraft(draft: ComposerDraft) {
    target = document.createElement('div');
    document.body.appendChild(target);
    host = mountHost();
    component = host;
    host.setDraft(draft);
    flushSync();
  }

  /** Let the composer's own follow-up to a change (the caret, the field's scroll) run. */
  async function settle() {
    flushSync();
    await tick();
    flushSync();
  }

  async function pressRemove(id: string) {
    const button = target.querySelector<HTMLElement>(
      `[data-quote-entry="${id}"] .composer-quote-remove`
    );
    if (!button) throw new Error(`no remove button for ${id}`);
    button.click();
    await settle();
  }

  async function undo() {
    expect(stack.undo()).toBe(true);
    await settle();
  }

  async function redo() {
    expect(stack.redo()).toBe(true);
    await settle();
  }

  function quoteIds(): string[] {
    return host.draft().quotes.map((q) => q.id);
  }

  beforeEach(() => {
    stack = installNativeUndoStack();
  });

  afterEach(() => {
    stack.uninstall();
  });

  it('puts the quote back where it was on undo, and takes it out again on redo', async () => {
    const draft = {
      quotes: [quote('a', '', 'first reply'), quote('b', 'lead-in', 'second reply'), quote('c')],
      text: 'after',
    };
    renderDraft(draft);

    await pressRemove('b');
    // The text before the removed quote joins what follows it.
    expect(host.draft()).toEqual({
      quotes: [quote('a', '', 'first reply'), quote('c', 'lead-in')],
      text: 'after',
    });

    await undo();
    expect(host.draft()).toEqual(draft);

    await redo();
    expect(quoteIds()).toEqual(['a', 'c']);
    expect(host.removedIds()).toEqual(['b', 'b']);

    await undo();
    expect(host.draft()).toEqual(draft);
  });

  it('undoes an unanswered quote removed with Backspace too, with the caret back in its gap', async () => {
    renderDraft({ quotes: [quote('a', '', 'a reply'), quote('b')], text: '' });
    focusWithCaret(gapOf('b'));

    gapOf('b').dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Backspace', bubbles: true, cancelable: true })
    );
    await settle();
    expect(quoteIds()).toEqual(['a']);
    expect(document.activeElement).toBe(mainInput());

    await undo();
    expect(quoteIds()).toEqual(['a', 'b']);
    expect(document.activeElement).toBe(gapOf('b'));

    await redo();
    expect(quoteIds()).toEqual(['a']);
    expect(document.activeElement).toBe(mainInput());
  });

  it('undoes in order with the typing before and after the removal', async () => {
    renderDraft({ quotes: [quote('a', '', 'a reply')], text: '' });

    stack.type(mainInput(), 'abc');
    await pressRemove('a');
    stack.type(mainInput(), 'def');
    await settle();
    expect(host.draft()).toEqual({ quotes: [], text: 'abcdef' });

    await undo();
    expect(host.draft()).toEqual({ quotes: [], text: 'abc' });
    await undo();
    expect(host.draft()).toEqual({ quotes: [quote('a', '', 'a reply')], text: 'abc' });
    await undo();
    expect(host.draft()).toEqual({ quotes: [quote('a', '', 'a reply')], text: '' });

    await redo();
    expect(host.draft()).toEqual({ quotes: [quote('a', '', 'a reply')], text: 'abc' });
    await redo();
    expect(host.draft()).toEqual({ quotes: [], text: 'abc' });
    await redo();
    expect(host.draft()).toEqual({ quotes: [], text: 'abcdef' });
    // Typing was the browser's own to undo and redo: the quote left once
    // by hand and once by redo, and nothing else touched it.
    expect(host.removedIds()).toEqual(['a', 'a']);
  });

  it('returns the caret to the reply it was taken from', async () => {
    renderDraft({ quotes: [quote('a', '', 'a reply'), quote('b')], text: '' });
    focusWithCaret(replyOf('a'));

    await pressRemove('a');
    // Focus in the removed block moved to the input its text joined.
    expect(document.activeElement).toBe(gapOf('b'));

    await undo();
    expect(document.activeElement).toBe(replyOf('a'));

    await redo();
    expect(document.activeElement).toBe(gapOf('b'));
  });

  it('leaves a caret that was elsewhere where it is, and scrolls the field to the restored block', async () => {
    renderDraft({ quotes: [quote('a'), quote('b', '', 'b reply')], text: 'typing here' });
    const fieldEl = layOutField();
    // The main input is in the field's view; the block will come back below it.
    mainInput().getBoundingClientRect = () => rect(210, 240);
    focusWithCaret(mainInput());
    await pressRemove('b');
    expect(document.activeElement).toBe(mainInput());

    stack.undo();
    flushSync();
    const block = target.querySelector<HTMLElement>('[data-quote-entry="b"] .composer-quote');
    if (!block) throw new Error('no block');
    block.getBoundingClientRect = () => rect(320, 380);
    await settle();

    expect(document.activeElement).toBe(mainInput());
    expect(fieldEl.scrollTop).toBe(79);
  });

  it('leaves the field scrolled where it was as the removal is recorded', async () => {
    renderDraft({ quotes: [quote('a', '', 'a reply')], text: 'typing here' });
    const fieldEl = layOutField();
    mainInput().getBoundingClientRect = () => rect(320, 350);
    focusWithCaret(mainInput());
    expect(fieldEl.scrollTop).toBe(49);
    // Scrolled back up to the quote, the caret still in the main input:
    // recording borrows focus and hands it back, which is not a move to
    // follow.
    fieldEl.scrollTop = 0;

    await pressRemove('a');

    expect(document.activeElement).toBe(mainInput());
    expect(fieldEl.scrollTop).toBe(0);
  });

  it('leaves the quote out when the parent refuses the undo, and restores it on a later one', async () => {
    const draft = { quotes: [quote('a', '', 'a reply')], text: '' };
    renderDraft(draft);
    await pressRemove('a');

    host.refuseUndo(true);
    await undo();
    expect(quoteIds()).toEqual([]);
    // Nothing came back, so there is nothing for redo to take out again.
    await redo();
    expect(host.removedIds()).toEqual(['a']);

    host.refuseUndo(false);
    await undo();
    expect(host.draft()).toEqual(draft);
  });

  it('withdraws an undo the parent is still holding on redo, rather than letting it land', async () => {
    const draft = { quotes: [quote('a', '', 'a reply')], text: '' };
    renderDraft(draft);
    await pressRemove('a');

    // The parent accepts the undo but has not put the quote back yet (the
    // pane's draft is loading): redo finds no quote in the box to take out.
    host.holdUndo(true);
    await undo();
    expect(quoteIds()).toEqual([]);
    await redo();

    host.landHeldUndo();
    await settle();
    expect(quoteIds()).toEqual([]);
    // Taken back, not removed a second time.
    expect(host.removedIds()).toEqual(['a']);

    // The removal is the step's to undo still.
    host.holdUndo(false);
    await undo();
    expect(host.draft()).toEqual(draft);
    await redo();
    expect(quoteIds()).toEqual([]);
    expect(host.removedIds()).toEqual(['a', 'a']);
  });

  it('takes the quote out again on redo once a held undo has landed', async () => {
    const draft = { quotes: [quote('a', '', 'a reply')], text: '' };
    renderDraft(draft);
    await pressRemove('a');

    host.holdUndo(true);
    await undo();
    host.landHeldUndo();
    await settle();
    expect(host.draft()).toEqual(draft);

    await redo();
    expect(quoteIds()).toEqual([]);
    expect(host.removedIds()).toEqual(['a', 'a']);
  });

  it('records nothing for a removal the parent gives no way back from', async () => {
    const props = render([quote('a')]);
    const anchor = target.querySelector<HTMLElement>('.native-undo-anchor');
    if (!anchor) throw new Error('no undo anchor');

    target.querySelector<HTMLElement>('.composer-quote-remove')!.click();
    await settle();

    expect(props.onRemoveQuote).toHaveBeenCalledWith('a');
    expect(anchor.textContent).toBe('');
    expect(stack.undo()).toBe(false);
  });
});
