import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';
import NoteSelectionReply from './NoteSelectionReply.svelte';
import type { DraftQuote, NewDraftQuote } from '../sessions/composerDraft';
import type { LiveQuoteReply } from '../sessions/replyDrafts';
import { viewport } from '../../shared/viewport.svelte';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

// The selection flow with nothing to confirm: the Reply button appears once
// a selection in the note has settled; with the chat composer beside the
// note a press adds the block there (`onAdd`), otherwise it opens a reply
// field whose block is already in the draft (`onReply`) and whose typing is
// handed on as it goes, so dismissing the field just closes it. By touch the
// tap that presses the button collapses the page selection first, and the
// captured quote has to survive that. On a phone the button is docked above
// the bottom bar instead of beside the selection, and the field is a sheet.
// The same field reopens on a block already in the draft (`editQuote`, from
// the passage the note marks for it), holding its reply, with a Remove
// button, and typing there edits that block (`onEdit`) rather than adding one.

const SETTLE_MS = 150;
const TOUCH_SETTLE_MS = 350;
const WHEEL_GRACE_MS = 300;
const DOCK_LEAVE_MS = 150;

let component: ReturnType<typeof mount> | null = null;
let host: HTMLElement;
let scrollEl: HTMLElement;
let contentEl: HTMLElement;

type Live = {
  id: string;
  setReply: Mock<LiveQuoteReply['setReply']>;
  finish: Mock<LiveQuoteReply['finish']>;
  remove: Mock<LiveQuoteReply['remove']>;
};

function liveReply(id = 'new-quote'): Live {
  return {
    id,
    setReply: vi.fn<LiveQuoteReply['setReply']>(),
    finish: vi.fn<LiveQuoteReply['finish']>(async () => {}),
    remove: vi.fn<LiveQuoteReply['remove']>(async () => {}),
  };
}

/** The component's exports, as NoteModal calls them. */
type Reply = {
  editQuote(quote: DraftQuote, range: Range, byKey?: boolean): boolean;
  closeQuote(quoteId: string): void;
};

/** Where the fixture's passage sits in its note (noteQuoteLocation). */
const PASSAGE_ANCHOR = { prefix: 'RolloutplanArchitecture', suffix: '', offset: 23 };

/** A block already in the draft, quoting the fixture's passage. */
const existing: DraftQuote = {
  id: 'q7',
  before: '',
  quote: 'The cache is invalidated on every write.',
  headings: ['Architecture'],
  reply: 'Why on every write?',
  anchor: PASSAGE_ANCHOR,
};

function passageRange(): Range {
  const range = document.createRange();
  range.selectNodeContents(contentEl.querySelector('#flow-p')!);
  return range;
}

function removeButton(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>('.selection-reply-remove');
}

/**
 * Mark the fixture's passage as the note does for `existing`
 * (noteQuoteHighlights): one span, which is the quote's keyboard stop.
 */
function markPassage(): HTMLElement {
  const paragraph = contentEl.querySelector('#flow-p')!;
  const stop = document.createElement('span');
  stop.className = 'note-quote-highlight';
  stop.setAttribute('data-quote-ids', existing.id);
  stop.setAttribute('data-quote-id', existing.id);
  stop.setAttribute('role', 'button');
  stop.tabIndex = 0;
  stop.append(...paragraph.childNodes);
  paragraph.append(stop);
  return stop;
}

/** The dialog around the note: focusable, as bits-ui's focus scope makes its content. */
function dialogHost(): HTMLElement {
  const dialog = document.createElement('div');
  dialog.tabIndex = -1;
  document.body.append(dialog);
  dialogs.push(dialog);
  return dialog;
}
const dialogs: HTMLElement[] = [];

function pressKey(el: Element, name: string): KeyboardEvent {
  const event = new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true });
  el.dispatchEvent(event);
  flushSync();
  return event;
}

function render(
  overrides: Partial<{
    enabled: boolean;
    chatOpen: boolean;
    chatVisible: boolean;
    sheetHost: HTMLElement | null;
    dockHost: HTMLElement | null;
    onDockedChange: (docked: boolean) => void;
    markedQuoteIds: string[];
    existingQuote: (quote: NewDraftQuote) => DraftQuote | null;
  }> = {}
) {
  host = document.createElement('div');
  scrollEl = document.createElement('div');
  scrollEl.style.position = 'relative';
  contentEl = document.createElement('div');
  contentEl.innerHTML = `
    <h1 id="title">Rollout plan</h1>
    <h2 id="arch">Architecture</h2>
    <p id="flow-p">The cache is invalidated on every write.</p>
  `;
  scrollEl.appendChild(contentEl);
  host.appendChild(scrollEl);
  document.body.appendChild(host);

  const live = liveReply();
  const editLive = liveReply(existing.id);
  const onAdd = vi.fn<(quote: NewDraftQuote) => Promise<boolean>>(async () => true);
  const onReply = vi.fn(() => live);
  const onEdit = vi.fn((_quote: DraftQuote) => editLive);
  const onActiveQuoteChange = vi.fn<(quoteId: string | null) => void>();
  const props = reactiveProps({
    scrollEl,
    contentEl,
    enabled: true,
    chatOpen: false,
    chatVisible: false,
    sheetHost: null as HTMLElement | null,
    dockHost: null as HTMLElement | null,
    onDockedChange: undefined as ((docked: boolean) => void) | undefined,
    ignoreHeading: (heading: Element) => heading.tagName === 'H1',
    onAdd,
    onReply,
    onEdit,
    existingQuote: undefined as ((quote: NewDraftQuote) => DraftQuote | null) | undefined,
    markedQuoteIds: [] as string[],
    onActiveQuoteChange,
    ...overrides,
  });
  component = mount(NoteSelectionReply, { target: scrollEl, props });
  flushSync();
  const reply = component as unknown as Reply;
  return { props, onAdd, onReply, live, onEdit, editLive, onActiveQuoteChange, reply };
}

/** Select the text of `#flow-p` and let the page know, as a drag or handles would. */
function selectPassage(from = 0, to?: number) {
  const text = contentEl.querySelector('#flow-p')!.firstChild as Text;
  const range = document.createRange();
  range.setStart(text, from);
  range.setEnd(text, to ?? text.length);
  const selection = document.getSelection()!;
  selection.removeAllRanges();
  selection.addRange(range);
  document.dispatchEvent(new Event('selectionchange'));
}

function collapseSelection() {
  document.getSelection()?.removeAllRanges();
  document.dispatchEvent(new Event('selectionchange'));
}

function settle(ms = SETTLE_MS) {
  vi.advanceTimersByTime(ms);
  flushSync();
}

function button(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>('.selection-reply-button');
}

function field(): HTMLTextAreaElement | null {
  return document.querySelector<HTMLTextAreaElement>('.selection-reply-input');
}

function dockButton(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>('.selection-reply-dock-button');
}

/** A phone: menus are sheets, and the dialog hosts the reply sheet and the docked button. */
function phoneHosts() {
  viewport.hasCoarsePointer = true;
  viewport.sheetMenus = true;
  viewport.showShortcutHints = false;
  const sheetHost = document.createElement('div');
  // Focusable, as bits-ui's focus scope makes the dialog's content.
  sheetHost.tabIndex = -1;
  const dockHost = document.createElement('div');
  document.body.append(sheetHost, dockHost);
  return {
    sheetHost,
    dockHost,
    cleanup() {
      sheetHost.remove();
      dockHost.remove();
    },
  };
}

function press(el: HTMLElement, pointerType = 'mouse') {
  el.dispatchEvent(
    new (window.PointerEvent ?? MouseEvent)('pointerdown', { bubbles: true, pointerType } as never)
  );
}

function click(el: HTMLElement) {
  el.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  flushSync();
}

async function flushAsync() {
  // Let the click's awaited `onAdd` settle without advancing fake timers.
  await Promise.resolve();
  await Promise.resolve();
  flushSync();
}

beforeEach(() => {
  vi.useFakeTimers();
  // jsdom lays nothing out: give the selected text one line box, so the
  // button has an end to sit under.
  Range.prototype.getClientRects = () =>
    [
      { left: 20, right: 180, top: 40, bottom: 60, width: 160, height: 20 },
    ] as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () =>
    ({ left: 20, right: 180, top: 40, bottom: 60, width: 160, height: 20 }) as DOMRect;
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      disconnect() {}
      unobserve() {}
    }
  );
  viewport.hasCoarsePointer = false;
  viewport.sheetMenus = false;
  viewport.showShortcutHints = true;
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  host.remove();
  for (const dialog of dialogs.splice(0)) dialog.remove();
  document.getSelection()?.removeAllRanges();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe('NoteSelectionReply button', () => {
  it('appears once a selection in the note has settled and goes when it collapses', () => {
    render();

    selectPassage();
    expect(button()).toBeNull();
    settle();
    expect(button()).not.toBeNull();

    collapseSelection();
    flushSync();
    expect(button()).toBeNull();
  });

  it('waits for the mouse to be released before showing', () => {
    render();

    press(contentEl, 'mouse');
    selectPassage();
    settle();
    expect(button()).toBeNull();

    document.dispatchEvent(new (window.PointerEvent ?? MouseEvent)('pointerup', { bubbles: true }));
    settle(0);
    expect(button()).not.toBeNull();
  });

  it('by touch shows after the handles have been still, with no pointer release', () => {
    viewport.hasCoarsePointer = true;
    render();

    press(contentEl, 'touch');
    selectPassage();
    settle(SETTLE_MS);
    expect(button()).toBeNull();
    settle(TOUCH_SETTLE_MS - SETTLE_MS);
    expect(button()).not.toBeNull();
  });
});

describe('NoteSelectionReply with the chat beside the note', () => {
  it('adds the quote straight to the composer and clears the selection', async () => {
    const { onAdd, onReply } = render({ chatVisible: true });
    selectPassage();
    settle();

    click(button()!);
    await flushAsync();

    expect(onAdd).toHaveBeenCalledWith({
      quote: 'The cache is invalidated on every write.',
      headings: ['Architecture'],
      reply: '',
      // Where it was selected, for the note to mark the passage by.
      anchor: PASSAGE_ANCHOR,
    });
    expect(onReply).not.toHaveBeenCalled();
    expect(field()).toBeNull();
    expect(button()).toBeNull();
    expect(document.getSelection()?.isCollapsed).toBe(true);
  });

  it('leaves a passage that is already quoted to `onAdd`, which has the composer', async () => {
    const existingQuote = vi.fn((_quote: NewDraftQuote) => existing as DraftQuote | null);
    const { onAdd, onEdit } = render({ chatVisible: true, chatOpen: true, existingQuote });
    selectPassage();
    settle();

    click(button()!);
    await flushAsync();

    expect(existingQuote).not.toHaveBeenCalled();
    expect(onEdit).not.toHaveBeenCalled();
    expect(onAdd).toHaveBeenCalledTimes(1);
    expect(field()).toBeNull();
  });

  it('keeps the button for another try when the block could not be added', async () => {
    const { onAdd } = render({ chatVisible: true });
    onAdd.mockResolvedValueOnce(false);
    selectPassage();
    settle();

    click(button()!);
    await flushAsync();

    expect(button()).not.toBeNull();
  });
});

describe('NoteSelectionReply reply field', () => {
  it('opens with the block already in the draft, types into it, and Enter closes it', () => {
    const { onReply, live } = render();
    selectPassage();
    settle();

    click(button()!);

    expect(onReply).toHaveBeenCalledWith({
      quote: 'The cache is invalidated on every write.',
      headings: ['Architecture'],
      reply: '',
      anchor: PASSAGE_ANCHOR,
    });
    const input = field();
    expect(input).not.toBeNull();
    // Nothing to confirm: no buttons in the field.
    expect(document.querySelectorAll('[data-slot="popover-content"] button')).toHaveLength(0);

    input!.value = 'Why on every write?';
    input!.dispatchEvent(new Event('input', { bubbles: true }));
    expect(live.setReply).toHaveBeenCalledWith('Why on every write?');

    const enter = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true });
    input!.dispatchEvent(enter);
    flushSync();
    expect(enter.defaultPrevented).toBe(true);
    expect(live.finish).toHaveBeenCalledTimes(1);
    expect(field()).toBeNull();
    expect(button()).toBeNull();
    expect(document.getSelection()?.isCollapsed).toBe(true);
  });

  it('lets Shift+Enter through for a new line', () => {
    const { live } = render();
    selectPassage();
    settle();
    click(button()!);

    const enter = new KeyboardEvent('keydown', {
      key: 'Enter',
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    });
    field()!.dispatchEvent(enter);
    flushSync();

    expect(enter.defaultPrevented).toBe(false);
    expect(live.finish).not.toHaveBeenCalled();
    expect(field()).not.toBeNull();
  });

  it('closes, keeping the block, when the chat opens beside the note', () => {
    const { props, live } = render();
    selectPassage();
    settle();
    click(button()!);
    expect(field()).not.toBeNull();

    props.chatOpen = true;
    props.chatVisible = true;
    flushSync();

    expect(live.finish).toHaveBeenCalledTimes(1);
    expect(field()).toBeNull();
  });

  it('keeps Escape from the dialog around it', () => {
    render();
    selectPassage();
    settle();
    click(button()!);
    const reachedDocument = vi.fn();
    document.addEventListener('keydown', reachedDocument);

    try {
      const escape = new KeyboardEvent('keydown', {
        key: 'Escape',
        bubbles: true,
        cancelable: true,
      });
      field()!.dispatchEvent(escape);
      flushSync();

      expect(escape.defaultPrevented).toBe(true);
      expect(reachedDocument).not.toHaveBeenCalled();
      expect(field()).toBeNull();
    } finally {
      document.removeEventListener('keydown', reachedDocument);
    }
  });

  it('closes when the note is scrolled with the wheel, once the trackpad has had time to settle', () => {
    const { live } = render();
    selectPassage();
    settle();
    click(button()!);

    // Inertia from the scroll before the selection: not a scroll away.
    scrollEl.dispatchEvent(new Event('wheel', { bubbles: true }));
    flushSync();
    expect(field()).not.toBeNull();

    settle(WHEEL_GRACE_MS);
    scrollEl.dispatchEvent(new Event('wheel', { bubbles: true }));
    flushSync();

    expect(live.finish).toHaveBeenCalledTimes(1);
    expect(field()).toBeNull();
  });

  it('writes what was typed when the note closes on an open field', () => {
    const { live } = render();
    selectPassage();
    settle();
    click(button()!);

    unmount(component!);
    component = null;

    expect(live.finish).toHaveBeenCalledTimes(1);
  });
});

describe('NoteSelectionReply reopened from a marked passage', () => {
  it('opens the same field by the passage, holding the reply written so far', () => {
    const { reply, onEdit, onReply, onAdd, editLive, onActiveQuoteChange } = render();

    expect(reply.editQuote(existing, passageRange())).toBe(true);
    flushSync();

    expect(onEdit).toHaveBeenCalledWith(existing);
    // Not a new block: nothing is added anywhere.
    expect(onReply).not.toHaveBeenCalled();
    expect(onAdd).not.toHaveBeenCalled();
    const input = field()!;
    expect(input.value).toBe('Why on every write?');
    // Anchored as a new reply's field is: hung from the button by the passage.
    expect(button()).not.toBeNull();
    expect(button()!.classList.contains('is-open')).toBe(true);
    expect(document.querySelector('[data-slot="popover-content"]')!.textContent).toContain(
      'The cache is invalidated on every write.'
    );
    expect(onActiveQuoteChange).toHaveBeenLastCalledWith('q7');

    input.value = 'Why on every write? Batch them.';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    expect(editLive.setReply).toHaveBeenCalledWith('Why on every write? Batch them.');

    input.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })
    );
    flushSync();
    expect(editLive.finish).toHaveBeenCalledTimes(1);
    expect(editLive.remove).not.toHaveBeenCalled();
    expect(field()).toBeNull();
    expect(button()).toBeNull();
    expect(onActiveQuoteChange).toHaveBeenLastCalledWith(null);
  });

  it('removes the block from its Remove button, without writing what was typed', () => {
    const { reply, editLive } = render();
    reply.editQuote(existing, passageRange());
    flushSync();

    const input = field()!;
    input.value = 'Never mind';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    click(removeButton()!);

    expect(editLive.remove).toHaveBeenCalledTimes(1);
    expect(editLive.finish).not.toHaveBeenCalled();
    expect(field()).toBeNull();
    expect(button()).toBeNull();
  });

  it("has no Remove button in a new reply's field", () => {
    const { live } = render();
    selectPassage();
    settle();
    click(button()!);

    expect(field()).not.toBeNull();
    expect(removeButton()).toBeNull();
    expect(live.remove).not.toHaveBeenCalled();
  });

  it("writes an open field's reply before reopening on another block", () => {
    const { reply, live, onEdit } = render();
    selectPassage();
    settle();
    click(button()!);
    expect(field()!.value).toBe('');

    reply.editQuote(existing, passageRange());
    flushSync();

    expect(live.finish).toHaveBeenCalledTimes(1);
    expect(onEdit).toHaveBeenCalledTimes(1);
    expect(field()!.value).toBe('Why on every write?');
    expect(removeButton()).not.toBeNull();
  });

  it('gives focus back to the mark a key press came from when a key closes the field', async () => {
    const { reply } = render();
    const stop = markPassage();

    reply.editQuote(existing, passageRange(), true);
    flushSync();
    const escape = pressKey(field()!, 'Escape');

    expect(escape.defaultPrevented).toBe(true);
    expect(field()).toBeNull();
    // Once the field has closed and let go of focus.
    await flushAsync();
    expect(document.activeElement).toBe(stop);
  });

  it('gives focus to the mark as the note has it now, when the note re-rendered under the field', async () => {
    const { reply } = render();
    const opened = markPassage();
    opened.focus();
    reply.editQuote(existing, passageRange(), true);
    flushSync();

    // An agent edited the note while the reply was being typed: `{@html}`
    // replaced every node, and the note marked the passage again.
    contentEl.innerHTML = contentEl.innerHTML.replace(/<\/?span[^>]*>/g, '');
    expect(opened.isConnected).toBe(false);
    const rebuilt = markPassage();

    pressKey(field()!, 'Enter');
    await flushAsync();

    expect(field()).toBeNull();
    expect(document.activeElement).toBe(rebuilt);
  });

  it('gives focus to the dialog when the mark a key press came from is gone', async () => {
    const dialog = dialogHost();
    const { reply } = render({ sheetHost: dialog });
    markPassage();
    reply.editQuote(existing, passageRange(), true);
    flushSync();

    // The passage was edited out of the note: nothing is marked any more.
    contentEl.querySelector('#flow-p')!.textContent = 'Rewritten.';

    pressKey(field()!, 'Escape');
    await flushAsync();

    expect(document.activeElement).toBe(dialog);
  });

  it('leaves focus alone when something other than a key closes the field', async () => {
    const { reply, props } = render();
    const stop = markPassage();

    reply.editQuote(existing, passageRange(), true);
    flushSync();
    props.chatOpen = true;
    flushSync();
    await flushAsync();

    expect(field()).toBeNull();
    expect(document.activeElement).not.toBe(stop);
  });

  it('leaves focus alone when a key closes a field a pointer press opened', async () => {
    const dialog = dialogHost();
    const { reply } = render({ sheetHost: dialog });
    const stop = markPassage();

    reply.editQuote(existing, passageRange());
    flushSync();
    pressKey(field()!, 'Enter');
    await flushAsync();

    expect(field()).toBeNull();
    expect(document.activeElement).not.toBe(stop);
    expect(document.activeElement).not.toBe(dialog);
  });

  it('gives focus to the dialog after Remove, whose mark goes with the block', async () => {
    const dialog = dialogHost();
    const { reply, editLive } = render({ sheetHost: dialog });
    const stop = markPassage();
    reply.editQuote(existing, passageRange(), true);
    flushSync();
    field()!.focus();

    click(removeButton()!);
    await flushAsync();

    expect(editLive.remove).toHaveBeenCalledTimes(1);
    expect(field()).toBeNull();
    // Not the mark, which is about to go, and not nowhere.
    expect(document.activeElement).not.toBe(stop);
    expect(document.activeElement).toBe(dialog);
  });

  it('closes when its block leaves the draft from elsewhere, and only then', async () => {
    const dialog = dialogHost();
    const { reply, editLive, onActiveQuoteChange } = render({ sheetHost: dialog });
    reply.editQuote(existing, passageRange());
    flushSync();
    field()!.focus();

    // Some other block went: nothing to do with this field.
    reply.closeQuote('another');
    flushSync();
    expect(field()).not.toBeNull();

    reply.closeQuote(existing.id);
    flushSync();
    await flushAsync();

    expect(field()).toBeNull();
    expect(button()).toBeNull();
    expect(onActiveQuoteChange).toHaveBeenLastCalledWith(null);
    // The block is gone, so there is nothing to remove; what was pending is
    // flushed as for any close (and lands nowhere).
    expect(editLive.remove).not.toHaveBeenCalled();
    expect(editLive.finish).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(dialog);

    // Nothing open: nothing to close.
    reply.closeQuote(existing.id);
    flushSync();
    expect(editLive.finish).toHaveBeenCalledTimes(1);
  });

  it('reopens the block a selection already has, instead of replying to it twice', () => {
    const existingQuote = vi.fn((_quote: NewDraftQuote) => existing as DraftQuote | null);
    const { onReply, onEdit, editLive } = render({ existingQuote });
    selectPassage();
    settle();

    click(button()!);

    // Asked with the passage as it was selected, anchor and all.
    expect(existingQuote).toHaveBeenCalledWith(
      expect.objectContaining({
        quote: 'The cache is invalidated on every write.',
        anchor: PASSAGE_ANCHOR,
      })
    );
    expect(onReply).not.toHaveBeenCalled();
    expect(onEdit).toHaveBeenCalledWith(existing);
    expect(field()!.value).toBe('Why on every write?');
    expect(removeButton()).not.toBeNull();

    pressKey(field()!, 'Enter');
    expect(editLive.finish).toHaveBeenCalledTimes(1);
    expect(field()).toBeNull();
    // The selection it was reopened from goes with the field, as for a new reply.
    expect(document.getSelection()!.isCollapsed).toBe(true);
  });

  it('adds a block as before for a selection no block has', () => {
    const existingQuote = vi.fn((_quote: NewDraftQuote) => null);
    const { onReply, onEdit } = render({ existingQuote });
    selectPassage(4, 9);
    settle();

    click(button()!);

    expect(existingQuote).toHaveBeenCalledTimes(1);
    expect(onReply).toHaveBeenCalledTimes(1);
    expect(onEdit).not.toHaveBeenCalled();
    expect(field()!.value).toBe('');
    expect(removeButton()).toBeNull();
  });

  it('closes, keeping the edit, when the chat opens', () => {
    const { reply, props, editLive } = render();
    reply.editQuote(existing, passageRange());
    flushSync();

    props.chatOpen = true;
    flushSync();

    expect(editLive.finish).toHaveBeenCalledTimes(1);
    expect(field()).toBeNull();
  });

  it('does not open while the note is hidden', () => {
    const { reply, onEdit } = render({ enabled: false });

    expect(reply.editQuote(existing, passageRange())).toBe(false);
    flushSync();

    expect(onEdit).not.toHaveBeenCalled();
    expect(field()).toBeNull();
  });

  it("on a phone opens the sheet, with no docked Reply row for a selection that isn't one", () => {
    const hosts = phoneHosts();
    try {
      const { reply, editLive } = render(hosts);

      expect(reply.editQuote(existing, passageRange())).toBe(true);
      flushSync();

      const sheet = hosts.sheetHost.querySelector('.selection-reply-sheet');
      expect(sheet).not.toBeNull();
      expect(document.activeElement).toBe(field());
      expect(field()!.value).toBe('Why on every write?');
      expect(dockButton()).toBeNull();

      click(removeButton()!);
      expect(editLive.remove).toHaveBeenCalledTimes(1);
      expect(hosts.sheetHost.querySelector('.selection-reply-sheet')).toBeNull();
    } finally {
      hosts.cleanup();
    }
  });
});

describe("NoteSelectionReply and the note's marks", () => {
  class FakeHighlight {
    constructor(public range: Range) {}
  }

  function stubHighlights() {
    const registry = new Map<string, FakeHighlight>();
    vi.stubGlobal('CSS', { highlights: registry });
    vi.stubGlobal('Highlight', FakeHighlight);
    return registry;
  }

  it('reports the block a new reply is written on', () => {
    const { onActiveQuoteChange } = render();
    selectPassage();
    settle();
    click(button()!);

    expect(onActiveQuoteChange).toHaveBeenLastCalledWith('new-quote');
  });

  it('paints the frozen selection only until the note marks the passage itself', () => {
    const registry = stubHighlights();
    const { props } = render();
    selectPassage();
    settle();
    click(button()!);
    expect(registry.has('note-reply-selection')).toBe(true);

    // The block landed in the draft and the note marked it.
    props.markedQuoteIds = ['new-quote'];
    flushSync();
    expect(registry.has('note-reply-selection')).toBe(false);

    // The mark went (the note re-rendered without the text): painted again.
    props.markedQuoteIds = [];
    flushSync();
    expect(registry.has('note-reply-selection')).toBe(true);
  });

  it('never paints over a mark it was reopened from', () => {
    const registry = stubHighlights();
    const { reply } = render({ markedQuoteIds: ['q7'] });

    reply.editQuote(existing, passageRange());
    flushSync();

    expect(field()).not.toBeNull();
    expect(registry.has('note-reply-selection')).toBe(false);
  });
});

describe('NoteSelectionReply by touch', () => {
  it('survives the tap collapsing the page selection before the button’s click', async () => {
    viewport.hasCoarsePointer = true;
    const { onAdd } = render({ chatVisible: true });
    selectPassage();
    settle(TOUCH_SETTLE_MS);
    const target = button()!;

    // iOS: the touch lands on the button, the selection goes, then the click.
    press(target, 'touch');
    collapseSelection();
    flushSync();
    expect(button()).toBe(target);

    click(target);
    await flushAsync();

    expect(onAdd).toHaveBeenCalledTimes(1);
    expect(onAdd.mock.calls[0][0]).toMatchObject({
      quote: 'The cache is invalidated on every write.',
    });
  });

  it('lets a press that never clicks drop the button again', () => {
    viewport.hasCoarsePointer = true;
    render();
    selectPassage();
    settle(TOUCH_SETTLE_MS);

    press(button()!, 'touch');
    collapseSelection();
    flushSync();
    expect(button()).not.toBeNull();

    settle(600);
    expect(button()).toBeNull();
  });

  it('opens the reply field as a sheet in the dialog, dismissed by a tap on its scrim', () => {
    const hosts = phoneHosts();
    try {
      const { live } = render(hosts);
      selectPassage();
      settle(TOUCH_SETTLE_MS);

      click(dockButton()!);

      const sheet = hosts.sheetHost.querySelector('.selection-reply-sheet');
      expect(sheet).not.toBeNull();
      expect(document.querySelector('[data-slot="popover-content"]')).toBeNull();
      expect(document.activeElement).toBe(field());
      expect(sheet!.textContent).not.toContain('Esc');

      const scrim = sheet!.querySelector<HTMLElement>('.selection-reply-scrim')!;
      press(scrim, 'touch');
      click(scrim);

      expect(live.finish).toHaveBeenCalledTimes(1);
      expect(hosts.sheetHost.querySelector('.selection-reply-sheet')).toBeNull();
    } finally {
      hosts.cleanup();
    }
  });

  it('hands focus back to the dialog when Return closes the sheet', () => {
    const hosts = phoneHosts();
    try {
      render(hosts);
      selectPassage();
      settle(TOUCH_SETTLE_MS);
      click(dockButton()!);
      expect(document.activeElement).toBe(field());

      field()!.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })
      );
      flushSync();

      expect(hosts.sheetHost.querySelector('.selection-reply-sheet')).toBeNull();
      expect(document.activeElement).toBe(hosts.sheetHost);
    } finally {
      hosts.cleanup();
    }
  });

  it('closes the sheet, keeping the block, when the chat opens over the note', () => {
    const hosts = phoneHosts();
    try {
      const { props, live } = render(hosts);
      selectPassage();
      settle(TOUCH_SETTLE_MS);
      click(dockButton()!);
      expect(hosts.sheetHost.querySelector('.selection-reply-sheet')).not.toBeNull();

      // Over the note, not beside it: `chatVisible` stays false on a phone.
      props.chatOpen = true;
      flushSync();

      expect(live.finish).toHaveBeenCalledTimes(1);
      expect(hosts.sheetHost.querySelector('.selection-reply-sheet')).toBeNull();
    } finally {
      hosts.cleanup();
    }
  });
});

describe('NoteSelectionReply docked on a phone', () => {
  it('docks a Reply row with the selected text above the bottom bar instead of a button by the selection', () => {
    const hosts = phoneHosts();
    const onDockedChange = vi.fn();
    try {
      const { onReply } = render({ ...hosts, onDockedChange });
      selectPassage();
      settle(TOUCH_SETTLE_MS);

      expect(button()).toBeNull();
      const docked = dockButton()!;
      expect(docked).not.toBeNull();
      expect(hosts.dockHost.contains(docked)).toBe(true);
      expect(docked.textContent).toContain('Reply to selection');
      expect(docked.textContent).toContain('The cache is invalidated on every write.');
      expect(onDockedChange).toHaveBeenLastCalledWith(true);

      click(docked);

      expect(onReply).toHaveBeenCalledWith({
        quote: 'The cache is invalidated on every write.',
        headings: ['Architecture'],
        reply: '',
        anchor: PASSAGE_ANCHOR,
      });
      expect(hosts.sheetHost.querySelector('.selection-reply-sheet')).not.toBeNull();
    } finally {
      hosts.cleanup();
    }
  });

  it('withdraws the docked row, after its leaving animation, when the selection collapses', () => {
    const hosts = phoneHosts();
    const onDockedChange = vi.fn();
    try {
      render({ ...hosts, onDockedChange });
      selectPassage();
      settle(TOUCH_SETTLE_MS);
      expect(dockButton()).not.toBeNull();

      collapseSelection();
      flushSync();

      const leaving = hosts.dockHost.querySelector('.selection-reply-dock');
      expect(leaving?.classList.contains('is-leaving')).toBe(true);
      expect(onDockedChange).toHaveBeenLastCalledWith(true);

      settle(DOCK_LEAVE_MS);

      expect(dockButton()).toBeNull();
      expect(onDockedChange).toHaveBeenLastCalledWith(false);
    } finally {
      hosts.cleanup();
    }
  });

  it('keeps the button by the selection on a coarse pointer without sheet menus', () => {
    viewport.hasCoarsePointer = true;
    viewport.sheetMenus = false;
    const dockHost = document.createElement('div');
    document.body.appendChild(dockHost);
    try {
      render({ dockHost });
      selectPassage();
      settle(TOUCH_SETTLE_MS);

      expect(dockButton()).toBeNull();
      expect(button()).not.toBeNull();
    } finally {
      dockHost.remove();
    }
  });
});

// The note components as text (Vite's raw import), for their stylesheets.
const componentSources = import.meta.glob('./*.svelte', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/**
 * A component's `<style>` block, put in the document as plain CSS. Svelte's
 * scoping class is not applied, so the rules match any element with the
 * class; jsdom lays nothing out but does resolve declared properties from
 * the stylesheets that match an element.
 */
function injectStyles(file: string): HTMLStyleElement {
  const source = componentSources[file];
  const css = source && /<style>([\s\S]*?)<\/style>/.exec(source)?.[1];
  if (!css) throw new Error(`no <style> in ${file}`);
  const style = document.createElement('style');
  style.textContent = css;
  document.head.appendChild(style);
  return style;
}

describe('the dock over the note’s bottom bar', () => {
  // NoteModal renders the dock (`.selection-dock`) whether or not a row is
  // in it, laid over the bottom bar at least the bar's size, so the bar
  // keeps its height when the row comes and the note's scroll box stays
  // put. Empty, the box must take no presses meant for the bar's buttons
  // under it — "Open chat", "Start note", "Unsent reply draft" — on any
  // platform; the row portalled into it takes its own.
  it('lets presses through an empty dock to the bar, and takes them on the docked row', () => {
    const styles = ['./NoteModal.svelte', './NoteSelectionReply.svelte'].map(injectStyles);
    const hosts = phoneHosts();
    const slot = document.createElement('div');
    slot.className = 'note-bottom';
    const dock = document.createElement('div');
    dock.className = 'selection-dock';
    slot.appendChild(dock);
    document.body.appendChild(slot);
    try {
      // Empty: at the slot's size, and no target.
      expect(getComputedStyle(dock).minHeight).toBe('100%');
      expect(getComputedStyle(dock).pointerEvents).toBe('none');

      render({ ...hosts, dockHost: dock });
      selectPassage();
      settle(TOUCH_SETTLE_MS);

      const row = dock.querySelector<HTMLElement>('.selection-reply-dock');
      expect(row).not.toBeNull();
      expect(row!.contains(dockButton())).toBe(true);
      // Docked: the row is pressable on its own; the box around it still isn't.
      expect(getComputedStyle(row!).pointerEvents).toBe('auto');
      expect(getComputedStyle(dock).pointerEvents).toBe('none');
    } finally {
      hosts.cleanup();
      slot.remove();
      for (const style of styles) style.remove();
    }
  });
});
