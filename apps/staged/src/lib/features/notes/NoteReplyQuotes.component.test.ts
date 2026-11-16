import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { store, toastError } = vi.hoisted(() => ({
  store: new Map<string, unknown>(),
  toastError: vi.fn(),
}));

vi.mock('../../shared/persistentStore', () => ({
  getStoreValue: vi.fn(async (key: string) => store.get(key)),
  setStoreValue: vi.fn(async (key: string, value: unknown) => {
    store.set(key, value);
  }),
  deleteStoreValue: vi.fn(async (key: string) => {
    store.delete(key);
  }),
  listStoreKeys: vi.fn(async () => [...store.keys()]),
  listenToStoreChanges: vi.fn(() => () => {}),
}));

vi.mock('svelte-sonner', () => ({ toast: { error: toastError } }));

import NoteReplyQuotes from './NoteReplyQuotes.svelte';
import { highlightNoteMatches, QUOTE_HIGHLIGHT_CLASS } from './noteQuoteHighlights';
import { saveDraft } from '../../shared/draftCache';
import { setStoreValue } from '../../shared/persistentStore';
import { clearHighlights, highlightMatches } from '../../shared/textHighlight';
import { viewport } from '../../shared/viewport.svelte';
import { composerDraftContent, type ComposerDraft } from '../sessions/composerDraft';
import {
  loadReplyDraft,
  registerReplyComposer,
  replyDraftKey,
  replyDraftPresence,
  replyDraftWriter,
  type ReplyComposerTarget,
} from '../sessions/replyDrafts';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

// The whole path, over the real draft (replyDrafts.ts, on a store kept in
// memory): a reply written to a selected passage leaves the passage marked
// for as long as its block is in the draft; pressing the mark, by pointer or
// key, reopens the same field on that block, so what is typed there changes
// it rather than adding another; removing or sending the block takes the
// mark away. With the chat beside the note the press goes to the composer's
// block instead.

const SETTLE_MS = 150;

const NOTE_HTML = `
  <h1 id="title">Rollout plan</h1>
  <h2 id="arch">Architecture</h2>
  <p id="flow-p">The cache is invalidated on every write.</p>
  <p id="link-p">Reads go through <a id="link" href="https://example.com/cache">the read cache</a> first.</p>
  <h2 id="rollout">Rollout</h2>
  <p id="again-p">Ship behind a flag. The cache is warmed before the switch.</p>
`;

let component: ReturnType<typeof mount> | null = null;
let host: HTMLElement;
let scrollEl: HTMLElement;
let contentEl: HTMLElement;
let unregister: (() => void) | null = null;
let sessionCount = 0;

let dialog: HTMLElement;

function render(overrides: Partial<{ chatOpen: boolean; chatVisible: boolean }> = {}) {
  host = document.createElement('div');
  // The dialog around the note: focusable, as bits-ui's focus scope makes it.
  dialog = document.createElement('div');
  dialog.tabIndex = -1;
  host.appendChild(dialog);
  scrollEl = document.createElement('div');
  scrollEl.style.position = 'relative';
  contentEl = document.createElement('div');
  contentEl.innerHTML = NOTE_HTML;
  scrollEl.appendChild(contentEl);
  host.appendChild(scrollEl);
  document.body.appendChild(host);

  // Its own draft each time: the presence tracker outlives a test.
  const sessionId = `note-session-${++sessionCount}`;
  const props = reactiveProps({
    sessionId,
    scrollEl,
    contentEl,
    renderKey: NOTE_HTML as unknown,
    refreshKey: 0 as unknown,
    enabled: true,
    chatOpen: false,
    chatVisible: false,
    sheetHost: dialog,
    ignoreHeading: (heading: Element) => heading.tagName === 'H1',
    ...overrides,
  });
  component = mount(NoteReplyQuotes, { target: scrollEl, props });
  flushSync();
  return { props, sessionId };
}

/** Let the draft's queued writes land and the note catch up with them. */
async function drain(ms = 0) {
  for (let round = 0; round < 3; round++) {
    await vi.advanceTimersByTimeAsync(ms);
    await replyDraftWriter.flush();
    await tick();
    flushSync();
  }
}

/** Select text of `selector`'s first text node, as a drag would. */
function select(selector: string, from = 0, to?: number) {
  const text = contentEl.querySelector(selector)!.firstChild as Text;
  const range = document.createRange();
  range.setStart(text, from);
  range.setEnd(text, to ?? text.length);
  const selection = document.getSelection()!;
  selection.removeAllRanges();
  selection.addRange(range);
  document.dispatchEvent(new Event('selectionchange'));
  vi.advanceTimersByTime(SETTLE_MS);
  flushSync();
}

function button(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>('.selection-reply-button');
}

function field(): HTMLTextAreaElement | null {
  return document.querySelector<HTMLTextAreaElement>('.selection-reply-input');
}

function click(el: Element, init: MouseEventInit = {}) {
  el.dispatchEvent(
    new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1, ...init })
  );
  flushSync();
}

function type(value: string) {
  const input = field()!;
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

function key(el: Element, name: string): KeyboardEvent {
  const event = new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true });
  el.dispatchEvent(event);
  flushSync();
  return event;
}

function marks(): HTMLElement[] {
  return [...contentEl.querySelectorAll<HTMLElement>(`.${QUOTE_HIGHLIGHT_CLASS}`)];
}

function markedText(): string {
  return marks()
    .map((mark) => mark.textContent)
    .join('');
}

/** Write a reply to `selector`'s text from the note, as the user does. */
async function replyTo(selector: string, reply: string, from = 0, to?: number) {
  select(selector, from, to);
  click(button()!);
  type(reply);
  key(field()!, 'Enter');
  await drain();
}

/** A draft put in the store by something other than this note view. */
async function storeDraft(sessionId: string, draft: ComposerDraft) {
  await saveDraft(replyDraftKey(sessionId), composerDraftContent(draft));
  await drain();
}

beforeEach(() => {
  vi.useFakeTimers();
  store.clear();
  toastError.mockClear();
  replyDraftPresence.start();
  // jsdom lays nothing out: give the passage one line box to anchor to.
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
  unregister?.();
  unregister = null;
  host.remove();
  document.getSelection()?.removeAllRanges();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe('NoteReplyQuotes marks', () => {
  it('marks a passage once a reply to it is in the draft', async () => {
    const { sessionId } = render();
    expect(marks()).toHaveLength(0);

    await replyTo('#flow-p', 'Why on every write?');

    expect(field()).toBeNull();
    expect(markedText()).toBe('The cache is invalidated on every write.');
    const draft = await loadReplyDraft(sessionId);
    expect(draft.quotes).toHaveLength(1);
    expect(draft.quotes[0].reply).toBe('Why on every write?');
    // Where in the note it was selected, for finding it again.
    expect(draft.quotes[0].anchor).toMatchObject({ prefix: 'RolloutplanArchitecture' });
    // Reachable and announced as something to press.
    const stop = marks()[0];
    expect(stop.getAttribute('role')).toBe('button');
    expect(stop.tabIndex).toBe(0);
    expect(stop.getAttribute('data-quote-id')).toBe(draft.quotes[0].id);
  });

  it('marks the occurrence that was selected when the note says the words twice', async () => {
    render();
    const text = contentEl.querySelector('#again-p')!.textContent!;
    const from = text.indexOf('The cache is');

    await replyTo('#again-p', 'Warmed how?', from, from + 'The cache is'.length);

    expect(markedText()).toBe('The cache is');
    expect(marks().every((mark) => mark.closest('#again-p') !== null)).toBe(true);
    expect(contentEl.querySelector('#flow-p .note-quote-highlight')).toBeNull();
  });

  it('puts the marks back when the note is rendered again', async () => {
    const { props } = render();
    await replyTo('#flow-p', 'Why?');

    // What `{@html}` does on a new render: the old nodes, marks and all, go.
    contentEl.innerHTML = NOTE_HTML;
    expect(marks()).toHaveLength(0);
    props.renderKey = `${NOTE_HTML} `;
    flushSync();

    expect(markedText()).toBe('The cache is invalidated on every write.');
  });

  it('marks what the draft already quotes when the note opens, anchor or not', async () => {
    const { sessionId } = render();

    // A block stored before quotes carried an anchor.
    await storeDraft(sessionId, {
      quotes: [
        {
          id: 'old',
          before: '',
          quote: 'Ship behind a flag.',
          headings: ['Rollout'],
          reply: 'Which flag?',
        },
      ],
      text: '',
    });

    expect(markedText()).toBe('Ship behind a flag.');
  });

  it('keeps the marks through a find-in-note pass, whose matches still reopen the reply', async () => {
    const { props } = render();
    await replyTo('#flow-p', 'Why?');

    // Find-in-note wraps its matches inside the marked text, and the note
    // re-marks around them (NoteModal bumps `refreshKey` on every pass)...
    const found = highlightMatches(contentEl, 'cache', 0);
    expect(found.total).toBe(3);
    props.refreshKey = 1;
    flushSync();
    expect(contentEl.querySelectorAll('mark.search-match')).toHaveLength(3);
    expect(found.elements.every((element) => element.isConnected)).toBe(true);
    expect(markedText()).toBe('The cache is invalidated on every write.');
    // A press lands on the innermost element: the mark's piece inside the match.
    const match = contentEl.querySelector<HTMLElement>('#flow-p mark.search-match')!;
    click(match.querySelector(`.${QUOTE_HIGHLIGHT_CLASS}`)!);
    expect(field()!.value).toBe('Why?');
    key(field()!, 'Escape');
    await drain();

    // ...and closing it flattens them again, after which the note re-marks.
    clearHighlights(contentEl);
    props.refreshKey = 2;
    flushSync();

    expect(contentEl.querySelector('mark')).toBeNull();
    expect(markedText()).toBe('The cache is invalidated on every write.');
    expect(contentEl.querySelector('#flow-p')!.textContent).toBe(
      'The cache is invalidated on every write.'
    );
  });

  it('lets a find-in-note match run across the edge of a mark', async () => {
    const { props } = render();
    const text = contentEl.querySelector('#flow-p')!.textContent!;
    const from = text.indexOf('cache is');
    await replyTo('#flow-p', 'Which cache?', from, from + 'cache is'.length);
    expect(markedText()).toBe('cache is');

    // A search as NoteModal runs one: over the note without its marks, which
    // it then has put back (`refreshKey`).
    let pass = 0;
    const search = (query: string) => {
      const found = highlightNoteMatches(scrollEl, contentEl, query);
      props.refreshKey = ++pass;
      flushSync();
      return found;
    };

    // "The cache" begins before the mark and ends inside it.
    const found = search('the cache');
    expect(found.total).toBe(2);
    expect(found.elements.every((element) => element.isConnected)).toBe(true);
    expect(found.elements[0].closest('#flow-p')).not.toBeNull();
    expect(markedText()).toBe('cache is');
    // The piece of the mark inside the match still reopens the reply.
    click(found.elements[0].querySelector(`.${QUOTE_HIGHLIGHT_CLASS}`)!);
    expect(field()!.value).toBe('Which cache?');
    key(field()!, 'Escape');
    await drain();

    // Typing on: each search starts from whole text again.
    expect(search('the cache is invalidated').total).toBe(1);
    expect(markedText()).toBe('cache is');

    search('');
    expect(contentEl.querySelector('mark')).toBeNull();
    expect(markedText()).toBe('cache is');
    expect(contentEl.querySelector('#flow-p')!.childNodes).toHaveLength(3);
  });

  it('drops the mark when the reply is sent and the draft cleared', async () => {
    const { sessionId } = render();
    await replyTo('#flow-p', 'Why?');
    expect(marks().length).toBeGreaterThan(0);

    await replyDraftWriter.clearNow(replyDraftKey(sessionId));
    await drain();

    expect(marks()).toHaveLength(0);
    expect(contentEl.querySelector('#flow-p')!.textContent).toBe(
      'The cache is invalidated on every write.'
    );
  });

  it('does not mark the same few words elsewhere once the quoted passage is rewritten', async () => {
    const { sessionId, props } = render();
    await replyTo('#flow-p', 'Which cache?', 0, 'The cache is'.length);
    expect(markedText()).toBe('The cache is');
    expect(contentEl.querySelector('#flow-p .note-quote-highlight')).not.toBeNull();

    // An agent rewrites the sentence. "The cache is" still stands under
    // Rollout, where the reply was never written.
    const rewritten = NOTE_HTML.replace(
      'The cache is invalidated on every write.',
      'Entries expire after an hour.'
    );
    expect(rewritten).toContain('The cache is warmed');
    contentEl.innerHTML = rewritten;
    props.renderKey = rewritten;
    flushSync();

    expect(marks()).toHaveLength(0);
    // The block is the user's to keep or remove, in the composer.
    expect((await loadReplyDraft(sessionId)).quotes.map((quote) => quote.reply)).toEqual([
      'Which cache?',
    ]);

    // The sentence comes back: so does its mark.
    contentEl.innerHTML = NOTE_HTML;
    props.renderKey = `${NOTE_HTML} `;
    flushSync();
    expect(markedText()).toBe('The cache is');
    expect(contentEl.querySelector('#flow-p .note-quote-highlight')).not.toBeNull();
  });

  it('leaves a quote the note no longer holds unmarked, and in the draft', async () => {
    const { sessionId } = render();

    await storeDraft(sessionId, {
      quotes: [
        {
          id: 'gone',
          before: '',
          quote: 'This sentence was edited out of the note.',
          headings: ['Architecture'],
          reply: 'Still relevant?',
          anchor: { prefix: 'Architecture', suffix: '', offset: 23 },
        },
      ],
      text: '',
    });

    expect(marks()).toHaveLength(0);
    expect(contentEl.innerHTML).toBe(NOTE_HTML);
    expect((await loadReplyDraft(sessionId)).quotes.map((quote) => quote.id)).toEqual(['gone']);
    expect(toastError).not.toHaveBeenCalled();
  });
});

describe('NoteReplyQuotes pressing a mark', () => {
  it('reopens the reply field on the same block, so editing it adds no other', async () => {
    const { sessionId } = render();
    await replyTo('#flow-p', 'Why on every write?');
    const [original] = (await loadReplyDraft(sessionId)).quotes;

    click(marks()[0]);

    const input = field()!;
    expect(input.value).toBe('Why on every write?');
    expect(document.querySelector('[data-slot="popover-content"]')!.textContent).toContain(
      'The cache is invalidated on every write.'
    );
    // The open reply's passage stays lit.
    expect(marks().every((mark) => mark.classList.contains('is-active'))).toBe(true);

    type('Why on every write? Batch them.');
    key(input, 'Enter');
    await drain();

    expect(field()).toBeNull();
    const draft = await loadReplyDraft(sessionId);
    expect(draft.quotes).toHaveLength(1);
    expect(draft.quotes[0]).toEqual({ ...original, reply: 'Why on every write? Batch them.' });
    // Still marked, and no longer lit.
    expect(markedText()).toBe('The cache is invalidated on every write.');
    expect(marks().some((mark) => mark.classList.contains('is-active'))).toBe(false);
  });

  it('writes an edit left in the field as it is typed, with no confirm', async () => {
    const { sessionId } = render();
    await replyTo('#flow-p', 'Why?');

    click(marks()[0]);
    type('Why? And when?');
    await drain(400);

    expect((await loadReplyDraft(sessionId)).quotes.map((quote) => quote.reply)).toEqual([
      'Why? And when?',
    ]);
  });

  it('removes the block, and its mark, from the reopened field', async () => {
    const { sessionId } = render();
    await replyTo('#flow-p', 'Why?');
    await replyTo('#again-p', 'Which flag?', 0, 'Ship behind a flag.'.length);
    expect((await loadReplyDraft(sessionId)).quotes).toHaveLength(2);

    click(contentEl.querySelector('#flow-p .note-quote-highlight')!);
    click(document.querySelector('.selection-reply-remove')!);
    await drain();

    expect(field()).toBeNull();
    expect(contentEl.querySelector('#flow-p .note-quote-highlight')).toBeNull();
    // The other quote keeps its block and its mark.
    expect(markedText()).toBe('Ship behind a flag.');
    expect((await loadReplyDraft(sessionId)).quotes.map((quote) => quote.reply)).toEqual([
      'Which flag?',
    ]);
    expect(toastError).not.toHaveBeenCalled();
  });

  it('opens from the keyboard, and gives focus back to the mark when a key closes it', async () => {
    render();
    await replyTo('#flow-p', 'Why?');
    const stop = marks()[0];
    stop.focus();

    for (const name of ['Enter', ' ']) {
      const event = key(stop, name);
      // Space is kept from scrolling the note.
      expect(event.defaultPrevented).toBe(true);
      expect(field()!.value).toBe('Why?');

      key(field()!, 'Escape');
      await drain();
      expect(field()).toBeNull();
      expect(document.activeElement).toBe(stop);
    }
  });

  it('gives focus back to the quote’s mark when the note re-rendered under the open field', async () => {
    const { props } = render();
    await replyTo('#flow-p', 'Why?');
    const opened = marks()[0];
    opened.focus();
    key(opened, 'Enter');
    expect(field()!.value).toBe('Why?');

    // An agent edits the note meanwhile: every node is replaced, and the
    // passage is marked again in the new markup.
    contentEl.innerHTML = NOTE_HTML;
    props.renderKey = `${NOTE_HTML} `;
    flushSync();
    expect(opened.isConnected).toBe(false);
    expect(field()!.value).toBe('Why?');

    key(field()!, 'Escape');
    await drain();

    expect(field()).toBeNull();
    const rebuilt = marks()[0];
    expect(rebuilt).not.toBe(opened);
    expect(rebuilt.getAttribute('role')).toBe('button');
    expect(document.activeElement).toBe(rebuilt);
  });

  it('gives focus to the dialog after Remove, when the mark goes', async () => {
    render();
    await replyTo('#flow-p', 'Why?');
    const stop = marks()[0];
    stop.focus();
    key(stop, 'Enter');
    field()!.focus();

    click(document.querySelector('.selection-reply-remove')!);
    await drain();

    expect(field()).toBeNull();
    expect(marks()).toHaveLength(0);
    expect(stop.isConnected).toBe(false);
    // Not left on an element that has gone.
    expect(document.activeElement).toBe(dialog);
  });

  it('reopens on the reply just typed when the mark is pressed before the write has landed', async () => {
    const { sessionId } = render();
    await replyTo('#flow-p', 'Why?');
    const stop = marks()[0];
    stop.focus();

    // Enter opens the field, Enter closes it, Enter opens it again: focus
    // goes back to the mark in between, and nothing waits for the draft.
    key(stop, 'Enter');
    type('Why? Batch them.');
    key(field()!, 'Enter');
    expect(field()).toBeNull();
    await Promise.resolve();
    await Promise.resolve();
    flushSync();
    expect(document.activeElement).toBe(marks()[0]);
    // The draft still shows the reply from before.
    expect(replyDraftPresence.quotes(sessionId)[0].reply).toBe('Why?');

    key(marks()[0], 'Enter');
    expect(field()!.value).toBe('Why? Batch them.');

    // What is typed next builds on it, and that is what the draft ends with.
    type('Why? Batch them. Per tenant.');
    key(field()!, 'Enter');
    await drain();
    expect((await loadReplyDraft(sessionId)).quotes.map((quote) => quote.reply)).toEqual([
      'Why? Batch them. Per tenant.',
    ]);

    // Once the draft has caught up it is what the field opens on again.
    await storeDraft(sessionId, {
      quotes: (await loadReplyDraft(sessionId)).quotes.map((quote) => ({
        ...quote,
        reply: 'Rewritten in the composer.',
      })),
      text: '',
    });
    click(marks()[0]);
    expect(field()!.value).toBe('Rewritten in the composer.');
  });

  it('keeps the reply just typed when the field is reopened and closed again untouched', async () => {
    const { sessionId } = render();
    await replyTo('#flow-p', 'Why?');

    click(marks()[0]);
    type('Why? Batch them.');
    key(field()!, 'Escape');
    click(marks()[0]);
    expect(field()!.value).toBe('Why? Batch them.');
    key(field()!, 'Escape');
    await drain();

    expect((await loadReplyDraft(sessionId)).quotes.map((quote) => quote.reply)).toEqual([
      'Why? Batch them.',
    ]);
    click(marks()[0]);
    expect(field()!.value).toBe('Why? Batch them.');
  });

  it('reopens on what the draft has when the reply typed could not be saved', async () => {
    const { sessionId } = render();
    await replyTo('#flow-p', 'Why?');

    click(marks()[0]);
    type('Why? Lost.');
    vi.mocked(setStoreValue).mockRejectedValue(new Error('store unavailable'));
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    try {
      key(field()!, 'Escape');
      await drain();
      expect(toastError).toHaveBeenCalledWith('Could not save your reply draft');
    } finally {
      vi.mocked(setStoreValue).mockImplementation(async (storeKey: string, value: unknown) => {
        store.set(storeKey, value);
      });
      consoleError.mockRestore();
    }

    expect((await loadReplyDraft(sessionId)).quotes[0].reply).toBe('Why?');
    click(marks()[0]);
    expect(field()!.value).toBe('Why?');
  });

  it('closes the field when its block leaves the draft from elsewhere', async () => {
    const { sessionId } = render();
    await replyTo('#flow-p', 'Why?');
    await replyTo('#again-p', 'Which flag?', 0, 'Ship behind a flag.'.length);
    const [first, second] = (await loadReplyDraft(sessionId)).quotes;

    click(contentEl.querySelector('#flow-p .note-quote-highlight')!);
    field()!.focus();
    expect(field()!.value).toBe('Why?');

    // Another block removed in another window: this field is not its.
    await storeDraft(sessionId, { quotes: [first], text: '' });
    expect(field()).not.toBeNull();
    expect(second.reply).toBe('Which flag?');

    // The draft sent from another window, this block with it.
    await replyDraftWriter.clearNow(replyDraftKey(sessionId));
    await drain();

    expect(field()).toBeNull();
    expect(marks()).toHaveLength(0);
    expect(document.activeElement).toBe(dialog);
    // Closing wrote nothing back: the draft stays sent.
    expect((await loadReplyDraft(sessionId)).quotes).toHaveLength(0);
    expect(replyDraftPresence.has(sessionId)).toBe(false);
    expect(toastError).not.toHaveBeenCalled();
  });

  it('keeps a new reply’s field open while its block is still on its way into the draft', async () => {
    const { sessionId } = render();
    select('#flow-p');
    click(button()!);
    // Nothing has landed yet: the draft does not show the block.
    expect(replyDraftPresence.quotes(sessionId)).toHaveLength(0);
    expect(field()).not.toBeNull();

    type('Why?');
    await drain(400);

    expect(field()!.value).toBe('Why?');
    expect((await loadReplyDraft(sessionId)).quotes.map((quote) => quote.reply)).toEqual(['Why?']);
  });

  it('leaves a drag across a mark to select text for a quote of its own', async () => {
    render();
    await replyTo('#flow-p', 'Why?');

    const mark = marks()[0];
    const range = document.createRange();
    range.setStart(mark.firstChild!, 4);
    range.setEnd(mark.firstChild!, 9);
    const selection = document.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
    click(mark);

    expect(field()).toBeNull();
  });

  it('leaves the second click of a double-click to select its word', async () => {
    render();
    await replyTo('#flow-p', 'Why?');

    click(marks()[0], { detail: 2 });

    expect(field()).toBeNull();
  });

  it('marks quoted text in a link without taking the link’s press', async () => {
    const { sessionId } = render();
    await storeDraft(sessionId, {
      quotes: [
        {
          id: 'linked',
          before: '',
          quote: 'Reads go through the read cache first.',
          headings: ['Architecture'],
          reply: 'All reads?',
        },
      ],
      text: '',
    });
    expect(markedText()).toBe('Reads go through the read cache first.');

    const inLink = contentEl.querySelector<HTMLElement>('#link .note-quote-highlight')!;
    expect(inLink.hasAttribute('data-quote-id')).toBe(false);
    expect(inLink.hasAttribute('tabindex')).toBe(false);
    const event = new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 });
    inLink.dispatchEvent(event);
    flushSync();
    expect(event.defaultPrevented).toBe(false);
    expect(field()).toBeNull();

    // The text around the link still reopens the reply.
    click(contentEl.querySelector('#link-p > .note-quote-highlight')!);
    expect(field()!.value).toBe('All reads?');
  });

  it('with the chat beside the note, puts the caret in the composer’s block instead', async () => {
    const { sessionId, props } = render();
    await replyTo('#flow-p', 'Why?');
    const [quote] = (await loadReplyDraft(sessionId)).quotes;

    const composer: ReplyComposerTarget = {
      addQuote: vi.fn(),
      setQuoteReply: vi.fn(),
      removeQuote: vi.fn(),
      focusQuoteReply: vi.fn(),
    };
    unregister = registerReplyComposer(sessionId, composer);
    props.chatOpen = true;
    props.chatVisible = true;
    flushSync();

    click(marks()[0]);

    expect(composer.focusQuoteReply).toHaveBeenCalledWith(quote.id);
    expect(field()).toBeNull();
  });
});

describe('NoteReplyQuotes replying to a passage that is already quoted', () => {
  it('reopens that reply rather than adding a second block on the same words', async () => {
    const { sessionId } = render();
    await replyTo('#flow-p', 'Why on every write?');

    // The same passage selected again, inside its mark.
    select('#flow-p .note-quote-highlight');
    click(button()!);

    expect(field()!.value).toBe('Why on every write?');
    expect(document.querySelector('.selection-reply-remove')).not.toBeNull();
    type('Why on every write? Batch them.');
    key(field()!, 'Enter');
    await drain();

    const draft = await loadReplyDraft(sessionId);
    expect(draft.quotes.map((quote) => quote.reply)).toEqual(['Why on every write? Batch them.']);
    expect(marks().some((mark) => mark.classList.contains('is-overlap'))).toBe(false);
  });

  it('still adds a block for a passage inside one that is quoted, or the same words elsewhere', async () => {
    const { sessionId } = render();
    await replyTo('#flow-p', 'Why?');
    const text = contentEl.querySelector('#again-p')!.textContent!;
    const from = text.indexOf('The cache is');
    await replyTo('#again-p', 'Warmed how?', from, from + 'The cache is'.length);

    // "The cache is" under Architecture: inside the first quote, and the
    // same words as the second, but neither's passage.
    select('#flow-p .note-quote-highlight', 0, 'The cache is'.length);
    click(button()!);
    expect(field()!.value).toBe('');
    type('Which cache?');
    key(field()!, 'Enter');
    await drain();

    expect((await loadReplyDraft(sessionId)).quotes.map((quote) => quote.reply)).toEqual([
      'Why?',
      'Warmed how?',
      'Which cache?',
    ]);
  });

  it('with the chat beside the note, puts the caret in the block the passage already has', async () => {
    const { sessionId, props } = render();
    await replyTo('#flow-p', 'Why?');
    const [quote] = (await loadReplyDraft(sessionId)).quotes;

    const composer: ReplyComposerTarget = {
      addQuote: vi.fn(),
      setQuoteReply: vi.fn(),
      removeQuote: vi.fn(),
      focusQuoteReply: vi.fn(),
    };
    unregister = registerReplyComposer(sessionId, composer);
    props.chatOpen = true;
    props.chatVisible = true;
    flushSync();

    select('#flow-p .note-quote-highlight');
    click(button()!);
    await drain();

    expect(composer.addQuote).not.toHaveBeenCalled();
    expect(composer.focusQuoteReply).toHaveBeenCalledWith(quote.id);
    // The selection is dealt with: the button goes, as after an add.
    expect(button()).toBeNull();

    // Any other passage is added to the composer, as before.
    select('#again-p', 0, 'Ship behind a flag.'.length);
    click(button()!);
    await drain();
    expect(composer.addQuote).toHaveBeenCalledTimes(1);
  });
});
