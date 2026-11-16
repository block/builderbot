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
/** How long after the field opens a wheel on the note is taken for a scroll away. */
const WHEEL_GRACE_MS = 300;

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
let headerButton: HTMLButtonElement;

function render(overrides: Partial<{ chatOpen: boolean; chatVisible: boolean }> = {}) {
  host = document.createElement('div');
  // The dialog, focusable as bits-ui's focus scope makes it, holding the
  // note and its header as NoteModal's does.
  dialog = document.createElement('div');
  dialog.tabIndex = -1;
  host.appendChild(dialog);
  headerButton = document.createElement('button');
  dialog.appendChild(headerButton);
  scrollEl = document.createElement('div');
  scrollEl.style.position = 'relative';
  contentEl = document.createElement('div');
  contentEl.innerHTML = NOTE_HTML;
  scrollEl.appendChild(contentEl);
  dialog.appendChild(scrollEl);
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

/** A mouse press on the page outside the open field, which bits-ui acts on 10ms later. */
function pressElsewhere() {
  headerButton.dispatchEvent(
    new (window.PointerEvent ?? MouseEvent)('pointerdown', {
      bubbles: true,
      pointerType: 'mouse',
      button: 0,
      clientX: 600,
      clientY: 600,
    } as never)
  );
  vi.advanceTimersByTime(20);
  flushSync();
}

type Rendered = ReturnType<typeof render>;

/** Every way the open reply field closes with the note still showing. */
const closingRoutes: Array<[string, (rendered: Rendered) => void]> = [
  ['Enter', () => key(field()!, 'Enter')],
  ['Escape', () => key(field()!, 'Escape')],
  ['a press elsewhere', () => pressElsewhere()],
  [
    'scrolling the note',
    () => {
      vi.advanceTimersByTime(WHEEL_GRACE_MS);
      scrollEl.dispatchEvent(new Event('wheel', { bubbles: true }));
      flushSync();
    },
  ],
  [
    'the chat opening over the note',
    ({ props }) => {
      props.chatOpen = true;
      flushSync();
    },
  ],
];

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

  it('keeps a mark after its neighbours are reworded, by the note’s edge or its section', async () => {
    const { props } = render();
    // The note's closing words, and a word it holds once, mid-sentence.
    const closing = contentEl.querySelector('#again-p')!.textContent!;
    await replyTo('#again-p', 'Which switch?', closing.indexOf('the switch.'));
    const flow = contentEl.querySelector('#flow-p')!.textContent!;
    const from = flow.indexOf('invalidated');
    await replyTo('#flow-p', 'By whom?', from, from + 'invalidated'.length);
    expect(markedText()).toBe('invalidatedthe switch.');

    // An agent rewords what stands around both.
    const reworded = NOTE_HTML.replace(
      'The cache is invalidated on every write.',
      'Entries get invalidated whenever a row changes.'
    ).replace('The cache is warmed before the switch.', 'Warm it, then flip the switch.');
    contentEl.innerHTML = reworded;
    props.renderKey = reworded;
    flushSync();

    expect(markedText()).toBe('invalidatedthe switch.');
    expect(contentEl.querySelector('#flow-p .note-quote-highlight')!.textContent).toBe(
      'invalidated'
    );
  });

  it('keeps keyboard focus on a quote’s mark when the marks are rebuilt', async () => {
    const { props } = render();
    await replyTo('#flow-p', 'Why?');
    const focused = () => document.activeElement as HTMLElement;
    const first = marks()[0];
    first.focus();

    // An agent edits the note: every node is replaced.
    contentEl.innerHTML = NOTE_HTML;
    expect(document.activeElement).toBe(document.body);
    props.renderKey = `${NOTE_HTML} `;
    flushSync();
    expect(focused()).not.toBe(first);
    expect(focused().getAttribute('role')).toBe('button');
    expect(focused().getAttribute('data-quote-id')).toBe(first.getAttribute('data-quote-id'));
    expect(focused().isConnected).toBe(true);

    // A find-in-note pass takes the marks out, and so does closing it.
    highlightNoteMatches(scrollEl, contentEl, 'cache');
    props.refreshKey = 1;
    flushSync();
    expect(focused()).toBe(marks()[0]);
    highlightNoteMatches(scrollEl, contentEl, '');
    props.refreshKey = 2;
    flushSync();
    expect(focused()).toBe(marks()[0]);
    expect(focused().isConnected).toBe(true);

    // Focus resting on the dialog around the note is where a removed stop
    // can leave it: nothing else took it, so it is still the mark's.
    contentEl.innerHTML = NOTE_HTML;
    dialog.focus();
    props.renderKey = NOTE_HTML;
    flushSync();
    expect(focused()).toBe(marks()[0]);
    expect(focused().getAttribute('data-quote-id')).toBe(first.getAttribute('data-quote-id'));

    // Another quote re-marks the note too, and its own mark takes nothing.
    await replyTo('#again-p', 'Which flag?', 0, 'Ship behind a flag.'.length);
    marks()[0].focus();
    await replyDraftWriter.clearNow(replyDraftKey(props.sessionId));
    await drain();
    // The quote is gone, and its mark with it: focus is not put elsewhere.
    expect(marks()).toHaveLength(0);
    expect(document.activeElement).toBe(document.body);
  });

  it('leaves focus where it went when the marks are rebuilt', async () => {
    const { props } = render();
    await replyTo('#flow-p', 'Why?');

    // Focus moved on from the mark before the note changed...
    marks()[0].focus();
    dialog.focus();
    contentEl.innerHTML = NOTE_HTML;
    props.renderKey = `${NOTE_HTML} `;
    flushSync();
    expect(document.activeElement).toBe(dialog);

    // ...or went somewhere of its own, outside the note, after the mark was
    // taken out.
    marks()[0].focus();
    contentEl.innerHTML = NOTE_HTML;
    headerButton.focus();
    props.renderKey = NOTE_HTML;
    flushSync();
    expect(document.activeElement).toBe(headerButton);
    // And is not brought back by a later pass.
    headerButton.blur();
    props.refreshKey = 1;
    flushSync();
    expect(document.activeElement).toBe(document.body);
  });

  it('keeps the mark under a resting pointer lit when the marks are rebuilt', async () => {
    const { props } = render();
    await replyTo('#flow-p', 'Why?');
    const over = (el: Element) => {
      el.dispatchEvent(new MouseEvent('pointerover', { bubbles: true }));
    };
    const lit = () => marks().filter((mark) => mark.classList.contains('is-hovered'));

    over(marks()[0]);
    expect(lit()).toHaveLength(1);

    // Find-in-note re-marks on every keystroke, under a pointer that is not moving.
    highlightNoteMatches(scrollEl, contentEl, 'cache');
    props.refreshKey = 1;
    flushSync();
    expect(marks().length).toBeGreaterThan(1);
    expect(lit()).toEqual(marks());

    // The pointer's next move is over a new span of the same quote, then away.
    over(marks()[1]);
    expect(lit()).toEqual(marks());
    over(contentEl.querySelector('#again-p')!);
    expect(lit()).toHaveLength(0);
    highlightNoteMatches(scrollEl, contentEl, '');
    props.refreshKey = 2;
    flushSync();
    expect(lit()).toHaveLength(0);
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
    // The open reply's passage stays lit, which is what shows what the
    // field replies to: the field does not repeat it, and no Reply button
    // stands beside it.
    expect(marks().every((mark) => mark.classList.contains('is-active'))).toBe(true);
    expect(document.querySelector('[data-slot="popover-content"]')!.textContent).not.toContain(
      'The cache is invalidated on every write.'
    );
    expect(button()).toBeNull();

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

  it.each(closingRoutes)(
    'deletes the block, and its mark, when the reopened field is emptied and closed by %s',
    async (_route, close) => {
      const rendered = render();
      const { sessionId } = rendered;
      await replyTo('#flow-p', 'Why?');
      await replyTo('#again-p', 'Which flag?', 0, 'Ship behind a flag.'.length);
      expect((await loadReplyDraft(sessionId)).quotes).toHaveLength(2);

      click(contentEl.querySelector('#flow-p .note-quote-highlight')!);
      expect(field()!.value).toBe('Why?');
      type('');
      close(rendered);
      await drain(400);

      expect(field()).toBeNull();
      expect(contentEl.querySelector('#flow-p .note-quote-highlight')).toBeNull();
      // The other quote keeps its block and its mark.
      expect(markedText()).toBe('Ship behind a flag.');
      expect((await loadReplyDraft(sessionId)).quotes.map((quote) => quote.reply)).toEqual([
        'Which flag?',
      ]);
      expect(toastError).not.toHaveBeenCalled();
    }
  );

  it.each(closingRoutes)(
    'leaves no block and no mark for a selection replied to with nothing, closed by %s',
    async (_route, close) => {
      const rendered = render();
      const { sessionId } = rendered;
      select('#flow-p');
      click(button()!);
      // The block is in the draft, and marked, while its field is open.
      await drain();
      expect(replyDraftPresence.quotes(sessionId)).toHaveLength(1);
      expect(markedText()).toBe('The cache is invalidated on every write.');

      close(rendered);
      await drain(400);

      expect(field()).toBeNull();
      expect(marks()).toHaveLength(0);
      expect((await loadReplyDraft(sessionId)).quotes).toEqual([]);
      expect(replyDraftPresence.has(sessionId)).toBe(false);
      expect(toastError).not.toHaveBeenCalled();
    }
  );

  it('leaves nothing behind when the field is closed before its block has even landed', async () => {
    const { sessionId } = render();
    select('#flow-p');
    click(button()!);
    expect(replyDraftPresence.quotes(sessionId)).toHaveLength(0);

    key(field()!, 'Escape');
    await drain(400);

    expect(marks()).toHaveLength(0);
    expect((await loadReplyDraft(sessionId)).quotes).toEqual([]);
    expect(replyDraftPresence.has(sessionId)).toBe(false);
  });

  it('leaves nothing behind when the note closes on a field nothing was typed in', async () => {
    const { sessionId } = render();
    select('#flow-p');
    click(button()!);
    await drain();
    expect((await loadReplyDraft(sessionId)).quotes).toHaveLength(1);

    unmount(component!);
    component = null;
    await drain(400);

    expect((await loadReplyDraft(sessionId)).quotes).toEqual([]);
    expect(replyDraftPresence.has(sessionId)).toBe(false);
  });

  it.each(closingRoutes)(
    'keeps a block that came with no reply and was not typed in, closed by %s',
    async (_route, close) => {
      const rendered = render();
      const { sessionId } = rendered;
      // Quoted in the chat composer and left for later.
      const unanswered = {
        id: 'from-composer',
        before: '',
        quote: 'The cache is invalidated on every write.',
        headings: ['Architecture'],
        reply: '',
      };
      await storeDraft(sessionId, { quotes: [unanswered], text: '' });
      expect(markedText()).toBe('The cache is invalidated on every write.');

      click(marks()[0]);
      expect(field()!.value).toBe('');
      close(rendered);
      await drain(400);

      expect(field()).toBeNull();
      expect(markedText()).toBe('The cache is invalidated on every write.');
      expect((await loadReplyDraft(sessionId)).quotes).toEqual([unanswered]);
    }
  );

  it('deletes a block that came with no reply once the user has typed in it and left it empty', async () => {
    const { sessionId } = render();
    await storeDraft(sessionId, {
      quotes: [
        {
          id: 'from-composer',
          before: '',
          quote: 'The cache is invalidated on every write.',
          headings: ['Architecture'],
          reply: '',
        },
      ],
      text: 'And a closing thought.',
    });

    click(marks()[0]);
    type('W');
    type(' ');
    key(field()!, 'Enter');
    await drain(400);

    expect(marks()).toHaveLength(0);
    // The quote goes; the text around it is the user's and stays.
    expect(await loadReplyDraft(sessionId)).toEqual({ quotes: [], text: 'And a closing thought.' });
  });

  it('keeps a reply that is not empty, however the field closes', async () => {
    const rendered = render();
    const { sessionId } = rendered;
    await replyTo('#flow-p', 'Why?');

    for (const [route, close] of closingRoutes) {
      click(marks()[0]);
      type(`Why? (${route})`);
      close(rendered);
      await drain(400);
      // The chat is closed again for the next way out.
      rendered.props.chatOpen = false;
      flushSync();

      expect(field()).toBeNull();
      expect(markedText()).toBe('The cache is invalidated on every write.');
      expect((await loadReplyDraft(sessionId)).quotes.map((quote) => quote.reply)).toEqual([
        `Why? (${route})`,
      ]);
    }
  });

  it('does not let a write still waiting on its pause bring a deleted quote back', async () => {
    const { sessionId } = render();
    const held = () => (component as unknown as { heldReplyCount(): number }).heldReplyCount();
    await replyTo('#flow-p', 'Why?');

    click(marks()[0]);
    type('Why? Batch them.');
    // Written after a pause; then more is typed, cleared, and the field
    // closed inside the next pause.
    await drain(400);
    type('Why? Batch them, or');
    type('');
    key(field()!, 'Enter');
    await drain(400);
    await drain(400);

    expect(marks()).toHaveLength(0);
    expect((await loadReplyDraft(sessionId)).quotes).toEqual([]);
    expect(replyDraftPresence.has(sessionId)).toBe(false);
    expect(held()).toBe(0);
  });

  it('does not delete a block given a reply elsewhere while its empty field was open', async () => {
    const { sessionId } = render();
    select('#flow-p');
    click(button()!);
    await drain();
    const [added] = (await loadReplyDraft(sessionId)).quotes;
    expect(added.reply).toBe('');

    // The reply is typed into the block in another window's composer.
    await storeDraft(sessionId, {
      quotes: [{ ...added, reply: 'Answered in the chat.' }],
      text: '',
    });
    key(field()!, 'Escape');
    await drain(400);

    expect((await loadReplyDraft(sessionId)).quotes.map((quote) => quote.reply)).toEqual([
      'Answered in the chat.',
    ]);
    // Still marked, and its field reopens on that reply.
    expect(markedText()).toBe('The cache is invalidated on every write.');
    click(marks()[0]);
    expect(field()!.value).toBe('Answered in the chat.');
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

  it('gives focus to the dialog when a key closes the field on an emptied reply, and the mark goes', async () => {
    render();
    await replyTo('#flow-p', 'Why?');
    const stop = marks()[0];
    stop.focus();
    key(stop, 'Enter');
    field()!.focus();

    type('');
    key(field()!, 'Enter');
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

  it('holds no reply once its field has closed and the draft has caught up or let the block go', async () => {
    const { sessionId } = render();
    const held = () => (component as unknown as { heldReplyCount(): number }).heldReplyCount();
    await replyTo('#flow-p', 'Why?');
    expect(held()).toBe(0);

    // Typed, and held until the write lands.
    click(marks()[0]);
    type('Why? Batch them.');
    expect(held()).toBe(1);
    key(field()!, 'Escape');
    await drain();
    expect(held()).toBe(0);

    // Typed, and the draft is sent from another window before the write:
    // the block the reply was for never shows it.
    click(marks()[0]);
    type('Why? Never shown.');
    await replyDraftWriter.clearNow(replyDraftKey(sessionId));
    await drain();
    expect(field()).toBeNull();
    expect(replyDraftPresence.quotes(sessionId)).toHaveLength(0);
    expect(held()).toBe(0);

    // The same for a new reply whose block is removed elsewhere as it is typed.
    select('#again-p', 0, 'Ship behind a flag.'.length);
    click(button()!);
    type('Which');
    await drain(400);
    expect(replyDraftPresence.quotes(sessionId)).toHaveLength(1);
    type('Which flag?');
    expect(held()).toBe(1);
    await storeDraft(sessionId, { quotes: [], text: '' });
    expect(field()).toBeNull();
    expect(held()).toBe(0);
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
