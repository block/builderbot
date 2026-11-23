import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { ImageDimensions, NoteVersion } from '../../types';

/**
 * The note viewer showing what a revision changed: the change bar, the
 * previous version opening beside the note in the chat's place, and Escape.
 * The dialog is the real one, so Escape goes through its own layer; the
 * window is wide enough for two panes unless a test narrows it. jsdom lays
 * nothing out, so where the markers and panes are is covered by
 * NoteChangeBar's and NotePreviousPane's own tests.
 */

const screen = vi.hoisted(() => {
  const state = { wide: true };
  // Before anything imports the viewport, which reads the window as it loads.
  window.matchMedia = ((query: string) => ({
    media: query,
    get matches() {
      return query === '(min-width: 1000px)' && state.wide;
    },
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
  return state;
});

const commands = vi.hoisted(() => ({
  getNotePreviousVersion: vi.fn<(noteId: string) => Promise<NoteVersion | null>>(),
  getMediaDimensions: vi.fn<(ids: string[]) => Promise<Record<string, ImageDimensions>>>(),
}));

vi.mock(
  '../sessions/PooledSessionChatPane.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('./NoteTransferMenu.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('./NoteReplyQuotes.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock(
  '../../shared/markdown/DiagramViewerModal.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock(
  '../../shared/markdown/MediaViewerModal.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
// Showing a note marks it read, through stores this test has no use for.
vi.mock('./noteReadTracker.svelte', () => ({
  NoteReadTracker: class {
    unread = false;
    canToggle = false;
    toggling = false;
    reset() {}
    show() {}
    observe() {}
    toggle() {}
  },
}));
// The dialog's stored width; there is no store here to read it from.
vi.mock('../../shared/persistentStore', () => ({
  getStoreValue: vi.fn(async () => undefined),
  setStoreValue: vi.fn(async () => {}),
  deleteStoreValue: vi.fn(async () => {}),
  listStoreKeys: vi.fn(async () => []),
  listenToStoreChanges: vi.fn(() => () => {}),
}));
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  listenToEvent: () => () => {},
}));
vi.mock('../../api/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/commands')>()),
  getMediaDimensions: commands.getMediaDimensions,
  getNotePreviousVersion: commands.getNotePreviousVersion,
  countAssistantMessagesAfter: async () => 0,
  markSessionChatRead: async () => {},
}));

import NoteModal from './NoteModal.svelte';
import { NOTE_DIFF_ACTIVE_ATTRIBUTE, NOTE_DIFF_ATTRIBUTE } from './noteDiffHighlights';
import { splitNoteMarkdown } from './noteMarkdown';
import {
  CURRENT_NOTE,
  fakeResizeObserver,
  HUNK_KINDS,
  PREVIOUS_NOTE,
} from './testStubs/noteDiffFixtures';
import { runSearchShortcut } from '../keyboard/searchTargets';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';
import { viewport } from '../../shared/viewport.svelte';

const MEDIA_ID = '3f2a9c1e-1234-4567-8123-123456789abc';
const NOTE_MEDIA_ID = '7b1d4e2a-9876-4abc-8def-0123456789ab';
const NOTE_DIALOG_WIDTH = 700;

const stored = (markdown: string) => {
  const { title, body } = splitNoteMarkdown(markdown);
  return { title, content: body };
};

const versionOf = (markdown: string, id = 'version-1'): NoteVersion => ({
  id,
  noteId: 'note-1',
  ...stored(markdown),
  savedAt: Date.now() - 5 * 60_000,
});

let modal: ReturnType<typeof mount> | null = null;

/** The dialog opens and closes over timers and frames; let them run, then render. */
async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 30));
  await tick();
  flushSync();
}

async function open(overrides: Record<string, unknown> = {}) {
  const onClose = vi.fn();
  const onChatOpenChange = vi.fn<(open: boolean) => void>();
  const props = reactiveProps({
    open: true,
    ...stored(CURRENT_NOTE),
    noteId: 'note-1',
    noteUpdatedAt: 2_000,
    sessionId: 'session-1' as string | null,
    chatOpen: false,
    onClose,
    onChatOpenChange,
    ...overrides,
  });
  const target = document.createElement('div');
  document.body.appendChild(target);
  modal = mount(NoteModal, { target, props });
  await settle();
  return { props, onClose, onChatOpenChange };
}

const dialog = () => document.querySelector<HTMLElement>('[data-slot="dialog-content"]');
const bar = () => document.querySelector('.note-change-bar');
const markers = () => [...document.querySelectorAll<HTMLButtonElement>('.note-change-marker')];
const marker = (kind: string) => markers().find((el) => el.dataset.kind === kind)!;
const previousPane = () => document.querySelector<HTMLElement>('.previous-pane');
const chatPane = () => document.querySelector('#note-modal-chat-pane');
const notePane = () => document.querySelector<HTMLElement>('#note-modal-note-pane')!;
const note = () => notePane().querySelector<HTMLElement>('.markdown-content')!;
const lit = (el: Element) =>
  [...el.querySelectorAll(`[${NOTE_DIFF_ACTIVE_ATTRIBUTE}]`)].map((block) => block.textContent);
const button = (label: string) =>
  document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;

async function press(el: HTMLElement) {
  el.click();
  await settle();
}

async function pressEscape(target: EventTarget = document) {
  target.dispatchEvent(
    new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
  );
  await settle();
}

const searchBar = () => document.querySelector<HTMLElement>('.search-bar');

/** Open find-in-note, as the search shortcut does, and return its field. */
async function openSearch() {
  runSearchShortcut('find');
  await settle();
  return searchBar()!.querySelector<HTMLInputElement>('input')!;
}

beforeEach(() => {
  document.body.innerHTML = '';
  screen.wide = true;
  viewport.canSplit = true;
  vi.stubGlobal('ResizeObserver', fakeResizeObserver().FakeResizeObserver);
  commands.getNotePreviousVersion.mockReset().mockResolvedValue(versionOf(PREVIOUS_NOTE));
  commands.getMediaDimensions.mockReset().mockResolvedValue({});
  // `computeLineDiff` reports each run.
  vi.spyOn(console, 'info').mockImplementation(() => {});
});

afterEach(async () => {
  if (modal) unmount(modal);
  modal = null;
  await settle();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('NoteModal change bar', () => {
  it('shows no bar for a note with no previous version', async () => {
    commands.getNotePreviousVersion.mockResolvedValue(null);
    await open();

    expect(commands.getNotePreviousVersion).toHaveBeenCalledWith('note-1');
    expect(note().textContent).toContain('The cache is filled when the app starts.');
    expect(bar()).toBeNull();
    expect(note().querySelector(`[${NOTE_DIFF_ATTRIBUTE}]`)).toBeNull();
  });

  it('asks for no previous version of a note that has no id yet', async () => {
    await open({ noteId: null });

    expect(commands.getNotePreviousVersion).not.toHaveBeenCalled();
    expect(bar()).toBeNull();
  });

  it('shows a marker for each change since the previous version', async () => {
    await open();

    expect(markers().map((el) => el.dataset.kind)).toEqual(HUNK_KINDS);
    // Nothing is tinted until a change is pointed at.
    expect(notePane().querySelector('.note-diff-all')).toBeNull();
    expect(lit(note())).toEqual([]);
    expect(previousPane()).toBeNull();
  });

  it('lights a change in the note while its marker is hovered', async () => {
    await open();

    marker('modified').dispatchEvent(new MouseEvent('pointerenter'));
    flushSync();
    expect(lit(note())).toEqual(['The cache is filled when the app starts.']);

    marker('modified').dispatchEvent(new MouseEvent('pointerleave'));
    flushSync();
    expect(lit(note())).toEqual([]);
  });

  it('draws a line where removed text was while its marker is hovered', async () => {
    await open();
    const gaps = () => [...notePane().querySelectorAll<HTMLElement>('.note-diff-gap')];
    expect(gaps()).toEqual([]);

    marker('removed').dispatchEvent(new MouseEvent('pointerenter'));
    flushSync();
    expect(gaps().map((gap) => gap.dataset.kind)).toEqual(['removed']);
    expect(gaps()[0].classList.contains('is-active')).toBe(true);

    marker('removed').dispatchEvent(new MouseEvent('pointerleave'));
    flushSync();
    expect(gaps()).toEqual([]);
  });

  it('compares with a previous version that holds an image once its size is known', async () => {
    const previous = `${PREVIOUS_NOTE}\n\n![Before](staged-media://${MEDIA_ID}.png)`;
    commands.getNotePreviousVersion.mockResolvedValue(versionOf(previous));
    commands.getMediaDimensions.mockResolvedValue({
      [MEDIA_ID]: { width: 640, height: 480, density: 2 },
    });
    await open();

    expect(commands.getMediaDimensions).toHaveBeenCalledExactlyOnceWith([MEDIA_ID]);
    // The image the revision dropped stood where its last paragraph was added:
    // one change, a modified one.
    expect(markers().map((el) => el.dataset.kind)).toEqual(['removed', 'modified', 'modified']);

    await press(marker('removed'));
    const image = previousPane()!.querySelector<HTMLImageElement>('.markdown-media img')!;
    expect(image.getAttribute('width')).toBe('320');
  });
});

describe('NoteModal media the previous version names', () => {
  const withImage = (markdown: string, id: string) =>
    `${markdown}\n\n![Figure](staged-media://${id}.png)`;
  const figureOf = (pane: Element, id: string) =>
    pane.querySelector<HTMLElement>(`figure[data-media-id="${id}"]`)!;
  /** The load failed; `error` does not bubble. */
  const fail = (figure: Element) => figure.querySelector('img')!.dispatchEvent(new Event('error'));

  it('shows a placeholder for an image the revision replaced, which is no longer stored', async () => {
    commands.getNotePreviousVersion.mockResolvedValue(
      versionOf(withImage(PREVIOUS_NOTE, MEDIA_ID))
    );
    await open(stored(withImage(CURRENT_NOTE, NOTE_MEDIA_ID)));
    await press(markers().at(-1)!);
    const figure = figureOf(previousPane()!, MEDIA_ID);
    expect(figure.hasAttribute('data-media-missing')).toBe(false);

    fail(figure);

    expect(figure.getAttribute('data-media-missing')).toBe('image');
    expect(figure.getAttribute('data-media-missing-label')).toBe('Image no longer available');
    // The block is still the changed one: marked, and lit by its marker.
    expect(figure.getAttribute(NOTE_DIFF_ATTRIBUTE)).toBe('modified');
    expect(figure.hasAttribute(NOTE_DIFF_ACTIVE_ATTRIBUTE)).toBe(true);
  });

  it('leaves an image both versions embed as it is, and the note’s own', async () => {
    commands.getNotePreviousVersion.mockResolvedValue(
      versionOf(withImage(PREVIOUS_NOTE, NOTE_MEDIA_ID))
    );
    await open(stored(withImage(CURRENT_NOTE, NOTE_MEDIA_ID)));
    await press(marker('modified'));

    fail(figureOf(previousPane()!, NOTE_MEDIA_ID));
    fail(figureOf(note(), NOTE_MEDIA_ID));

    expect(document.querySelector('[data-media-missing]')).toBeNull();
  });
});

describe('NoteModal image sizes', () => {
  it('does not hold the note back for the images of its previous version', async () => {
    // Each lookup waits for the test, as a slow one would.
    const lookups: {
      ids: string[];
      answer: (found: Record<string, ImageDimensions>) => void;
    }[] = [];
    commands.getMediaDimensions.mockImplementation(
      (ids) => new Promise((answer) => lookups.push({ ids, answer }))
    );
    const withImage = (markdown: string, id: string) =>
      `${markdown}\n\n![Figure](staged-media://${id}.png)`;
    commands.getNotePreviousVersion.mockResolvedValue(
      versionOf(withImage(PREVIOUS_NOTE, MEDIA_ID))
    );

    // The previous version arrives while the note's own lookup is out.
    await open(stored(withImage(CURRENT_NOTE, NOTE_MEDIA_ID)));
    // Its image is asked for on its own: the note's is not asked for twice.
    expect(lookups.map((lookup) => lookup.ids)).toEqual([[NOTE_MEDIA_ID], [MEDIA_ID]]);
    expect(notePane().querySelector('.markdown-content')).toBeNull();

    // The note renders on its own lookup's answer, the other still out.
    lookups[0].answer({ [NOTE_MEDIA_ID]: { width: 640, height: 480, density: 2 } });
    await settle();
    expect(note().querySelector('.markdown-media img')!.getAttribute('width')).toBe('320');
    expect(bar()).toBeNull();

    lookups[1].answer({ [MEDIA_ID]: { width: 200, height: 100, density: 1 } });
    await settle();
    expect(markers().length).toBeGreaterThan(0);
    expect(lookups).toHaveLength(2);
  });
});

describe('NoteModal previous version', () => {
  it('opens the previous version beside the note on a marker press, in the chat’s place', async () => {
    const { onChatOpenChange } = await open({ chatOpen: true });
    expect(chatPane()).not.toBeNull();
    expect(dialog()!.style.width).toBe(`${NOTE_DIALOG_WIDTH + 380}px`);

    await press(marker('modified'));

    const pane = previousPane()!;
    expect(pane.textContent).toContain('Previous version');
    expect(pane.textContent).toContain('saved 5m ago');
    expect(pane.querySelector('.markdown-content')!.textContent).toContain(
      'The cache is warmed when the app starts.'
    );
    // The chat closed for it, and the host was told.
    expect(chatPane()).toBeNull();
    expect(onChatOpenChange).toHaveBeenLastCalledWith(false);
    // Two note columns, both named, and every change tinted in both.
    expect(dialog()!.style.width).toBe(`${NOTE_DIALOG_WIDTH * 2}px`);
    expect(notePane().querySelector('.note-diff-pane-header')!.textContent).toContain(
      'Current version'
    );
    expect(notePane().querySelector('.modal-content')!.classList.contains('note-diff-all')).toBe(
      true
    );
    // The pressed change is lit in both versions.
    expect(lit(note())).toEqual(['The cache is filled when the app starts.']);
    expect(lit(pane)).toEqual(['The cache is warmed when the app starts.']);
    expect(marker('modified').getAttribute('aria-pressed')).toBe('true');
  });

  it('closes the previous version when the chat opens', async () => {
    const { onChatOpenChange } = await open();
    await press(marker('modified'));
    expect(previousPane()).not.toBeNull();

    await press(button('Show chat pane'));

    expect(previousPane()).toBeNull();
    expect(chatPane()).not.toBeNull();
    expect(onChatOpenChange).toHaveBeenLastCalledWith(true);
    expect(lit(note())).toEqual([]);
    expect(dialog()!.style.width).toBe(`${NOTE_DIALOG_WIDTH + 380}px`);
  });

  it('closes the previous version when the host opens the chat', async () => {
    const { props } = await open();
    await press(marker('modified'));

    props.chatOpen = true;
    await settle();

    expect(previousPane()).toBeNull();
    expect(chatPane()).not.toBeNull();
  });

  it('closes the previous version from its header and keeps the note', async () => {
    const { onClose } = await open();
    await press(marker('added'));

    await press(button('Close previous version'));

    expect(previousPane()).toBeNull();
    expect(lit(note())).toEqual([]);
    expect(notePane().querySelector('.note-diff-pane-header')).toBeNull();
    expect(dialog()!.style.width).toBe(`${NOTE_DIALOG_WIDTH}px`);
    expect(markers()).toHaveLength(3);
    expect(onClose).not.toHaveBeenCalled();
  });

  it('closes the previous version on Escape and the note on the next', async () => {
    const { onClose } = await open();
    await press(marker('modified'));
    expect(previousPane()).not.toBeNull();

    await pressEscape();

    expect(previousPane()).toBeNull();
    expect(lit(note())).toEqual([]);
    expect(dialog()).not.toBeNull();
    expect(onClose).not.toHaveBeenCalled();

    await pressEscape();

    expect(onClose).toHaveBeenCalledOnce();
  });

  it('closes only the search on Escape in the find field, and keeps the previous version', async () => {
    const { onClose } = await open();
    await press(marker('modified'));
    const field = await openSearch();

    await pressEscape(field);

    expect(searchBar()).toBeNull();
    expect(previousPane()).not.toBeNull();
    expect(lit(note())).toEqual(['The cache is filled when the app starts.']);
    expect(onClose).not.toHaveBeenCalled();

    // The next Escape is the previous version's, and the one after the note's.
    await pressEscape();
    expect(previousPane()).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    await pressEscape();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('closes the search and the note on one Escape in the find field when nothing of the diff is open', async () => {
    const { onClose } = await open();
    const field = await openSearch();

    await pressEscape(field);

    expect(searchBar()).toBeNull();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('closes only the search on Escape in the find field while a change is lit without a second pane', async () => {
    screen.wide = false;
    viewport.canSplit = false;
    const { onClose } = await open();
    await press(marker('modified'));
    const field = await openSearch();

    await pressEscape(field);

    expect(searchBar()).toBeNull();
    expect(lit(note())).toEqual(['The cache is filled when the app starts.']);
    expect(onClose).not.toHaveBeenCalled();
  });

  it('closes the search and the note on one Escape again once the previous version is closed', async () => {
    const { onClose } = await open();
    await press(marker('modified'));
    await pressEscape();
    expect(previousPane()).toBeNull();
    const field = await openSearch();

    await pressEscape(field);

    expect(searchBar()).toBeNull();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('closes the note on Escape when nothing of the diff is open', async () => {
    const { onClose } = await open();

    await pressEscape();

    expect(onClose).toHaveBeenCalledOnce();
  });

  it('keeps the change lit without a second pane where the window is too narrow', async () => {
    screen.wide = false;
    viewport.canSplit = false;
    const { onClose } = await open();

    await press(marker('modified'));

    expect(previousPane()).toBeNull();
    expect(lit(note())).toEqual(['The cache is filled when the app starts.']);
    expect(dialog()!.style.width).toBe(`${NOTE_DIALOG_WIDTH}px`);

    // Escape lets go of it before it closes the note.
    await pressEscape();
    expect(lit(note())).toEqual([]);
    expect(onClose).not.toHaveBeenCalled();
  });

  it('starts over when the note is revised under an open previous version', async () => {
    const { props } = await open();
    await press(marker('modified'));
    expect(previousPane()).not.toBeNull();

    // What was the note is now its previous version.
    const revised = `${CURRENT_NOTE}\n\nOne more paragraph.`;
    commands.getNotePreviousVersion.mockResolvedValue(versionOf(CURRENT_NOTE, 'version-2'));
    Object.assign(props, stored(revised), { noteUpdatedAt: 3_000 });
    await settle();

    expect(previousPane()).toBeNull();
    expect(markers().map((el) => el.dataset.kind)).toEqual(['added']);
    expect(lit(note())).toEqual([]);
  });

  it('keeps the previous version open through a write that left the text alone', async () => {
    const { props } = await open();
    await press(marker('modified'));

    props.noteUpdatedAt = 3_000;
    await settle();

    expect(commands.getNotePreviousVersion).toHaveBeenCalledTimes(2);
    expect(previousPane()).not.toBeNull();
    expect(lit(note())).toEqual(['The cache is filled when the app starts.']);
  });
});
