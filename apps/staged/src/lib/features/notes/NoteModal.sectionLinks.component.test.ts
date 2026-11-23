import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * Section links in the note viewer: a `[text](#heading-slug)` link scrolls
 * the note it is in to that heading. The viewer is the one place a note body
 * is rendered (project notes, repo notes and the child notes opened over a
 * project note all come through it), so two can be mounted at once.
 *
 * jsdom has no layout and no `Element.scrollTo`: these show which note's
 * scroll box is told to scroll, where focus goes, and that the other link
 * kinds still reach their own handling. The scroll itself, and the look of
 * the landing, need the real app.
 */

vi.mock('$lib/components/ui/dialog', async () => {
  const { dialogModuleMock } = await import('../sessions/testStubs/dialogMock');
  return dialogModuleMock();
});
vi.mock(
  '../sessions/PooledSessionChatPane.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('./NoteTransferMenu.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock(
  '../../shared/markdown/DiagramViewerModal.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock(
  '../../shared/markdown/MediaViewerModal.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
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
  getMediaDimensions: async () => ({}),
}));

import NoteModal from './NoteModal.svelte';
import type { HashtagClickInfo } from '../references/referenceHistory.svelte';
import type { HashtagItem } from '../../types';

const BODY = [
  'Jump to the [steps](#ordered-steps), or see [the site](https://example.com/docs#ordered-steps).',
  '',
  'Related: #note:abc123',
  '',
  '## Ordered steps',
  '',
  '| Step | See |',
  '| --- | --- |',
  '| First | [the title](#TITLE) |',
  '',
  '### See #note:abc123',
  '',
  'Back to [the related section](#see-noteabc123) or [the appendix](#appendix).',
  '',
  '<a name="appendix"></a>Appendix text.',
].join('\n');

const NOTE_ITEM: HashtagItem = {
  type: 'note',
  id: 'abc123',
  title: 'Related note',
  color: '--note-color',
  bgColor: '--note-bg',
};

async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

interface MountedNote {
  root: HTMLElement;
  scrollTo: ReturnType<typeof vi.fn>;
  hashtagClicks: HashtagClickInfo[];
  link(href: string): HTMLAnchorElement;
  heading(text: string): HTMLElement;
}

describe('NoteModal section links', () => {
  const modals: ReturnType<typeof mount>[] = [];

  beforeEach(() => {
    document.body.innerHTML = '';
  });

  afterEach(() => {
    modals.splice(0).forEach((modal) => unmount(modal));
    vi.restoreAllMocks();
  });

  async function mountNote(title: string): Promise<MountedNote> {
    const root = document.createElement('div');
    document.body.appendChild(root);
    const hashtagClicks: HashtagClickInfo[] = [];
    modals.push(
      mount(NoteModal, {
        target: root,
        props: {
          open: true,
          title,
          content: BODY,
          onClose: () => {},
          hashtagItems: [NOTE_ITEM],
          onHashtagClick: (click: HashtagClickInfo) => hashtagClicks.push(click),
        },
      })
    );
    await settle();

    const scroller = root.querySelector<HTMLElement>('.modal-content')!;
    const scrollTo = vi.fn();
    scroller.scrollTo = scrollTo as unknown as Element['scrollTo'];
    return {
      root,
      scrollTo,
      hashtagClicks,
      link(href) {
        return [...root.querySelectorAll<HTMLAnchorElement>('.markdown-content a')].find(
          (a) => a.getAttribute('href') === href
        )!;
      },
      heading(text) {
        return [...root.querySelectorAll<HTMLElement>('.markdown-content :is(h1, h2, h3)')].find(
          (h) => h.textContent === text
        )!;
      },
    };
  }

  /**
   * Click and report whether the viewer prevented the default (the webview
   * navigating). Read at the body: Svelte delegates `onclick` to the mount
   * root, so the viewer's handler has run by the time the click gets there.
   */
  function click(el: HTMLElement): boolean {
    let prevented = false;
    document.body.addEventListener(
      'click',
      (e) => {
        prevented = e.defaultPrevented;
        // jsdom cannot load a page, so nothing is left to try.
        e.preventDefault();
      },
      { once: true }
    );
    el.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    return prevented;
  }

  it('scrolls a note to its own heading when another open note has the same one', async () => {
    const parent = await mountNote('Title');
    const child = await mountNote('Title');

    expect(click(child.link('#ordered-steps'))).toBe(true);
    expect(child.scrollTo).toHaveBeenCalledTimes(1);
    expect(child.scrollTo.mock.calls[0][0]).toMatchObject({ behavior: 'smooth' });
    expect(parent.scrollTo).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(child.heading('Ordered steps'));

    expect(click(parent.link('#ordered-steps'))).toBe(true);
    expect(parent.scrollTo).toHaveBeenCalledTimes(1);
    expect(child.scrollTo).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(parent.heading('Ordered steps'));
  });

  it('follows a link in a table cell back to the title the note is rendered under', async () => {
    const note = await mountNote('Title');
    const link = note.link('#TITLE');
    expect(link.closest('td')).not.toBeNull();

    expect(click(link)).toBe(true);

    expect(note.scrollTo).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(note.heading('Title'));
  });

  it('finds a heading by the reference written in it, whatever title the badge shows', async () => {
    const note = await mountNote('Title');
    // The badge is labelled with the referenced note's title.
    const heading = note.heading('See Related note');
    expect(heading.querySelector('[data-hashtag-ref="#note:abc123"]')).not.toBeNull();

    expect(click(note.link('#see-noteabc123'))).toBe(true);

    expect(note.scrollTo).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(heading);
  });

  it('follows a link to an anchor the author named', async () => {
    const note = await mountNote('Title');
    const anchor = note.root.querySelector<HTMLElement>('.markdown-content a[name="appendix"]');
    expect(anchor).not.toBeNull();

    expect(click(note.link('#appendix'))).toBe(true);

    expect(note.scrollTo).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(anchor);
  });

  it('still opens an http link outside the app, and a hashtag reference as a reference', async () => {
    const opened = vi.spyOn(window, 'open').mockReturnValue({ opener: window } as Window);
    const note = await mountNote('Title');

    expect(click(note.link('https://example.com/docs#ordered-steps'))).toBe(true);
    expect(opened).toHaveBeenCalledWith('https://example.com/docs#ordered-steps', '_blank');

    const badge = note.root.querySelector<HTMLElement>('[data-hashtag-ref="#note:abc123"]')!;
    click(badge);
    expect(note.hashtagClicks.map((hit) => hit.ref)).toEqual(['#note:abc123']);

    expect(note.scrollTo).not.toHaveBeenCalled();
  });
});
