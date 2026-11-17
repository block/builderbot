import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest';

/**
 * A video in a note. The viewer renders the note with `{@html}`, which
 * creates the `<video>` in a template's document; WebKit plays such a video
 * with a blank picture, so the viewer swaps it for one the page's document
 * made (shared/markdown/pageVideos.ts). Which document made the element is
 * all jsdom can show; the picture is checked in a capture of the real app.
 */

const MEDIA_ID = '3f2a9c1e-1234-4567-8123-123456789abc';
const OTHER_ID = '7c1d2e3f-89ab-4cde-8f01-23456789abcd';

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
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';
import type { HashtagItem } from '../../types';

const note = (id: string) =>
  `# Demo\n\nThe flow, recorded:\n\n![Walkthrough](staged-media://${id}.mp4)`;

/** The same elements, not merely equal ones: `toEqual` compares nodes by their markup. */
function expectSame(actual: Node[], expected: Node[]) {
  expect(actual).toHaveLength(expected.length);
  actual.forEach((node, i) => expect(node).toBe(expected[i]));
}

async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

describe('NoteModal video in a note', () => {
  let modal: ReturnType<typeof mount> | null = null;
  let importNode: MockInstance<Document['importNode']>;

  beforeEach(() => {
    document.body.innerHTML = '';
    // jsdom only reports that it has no `load`.
    vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(() => {});
    importNode = vi.spyOn(document, 'importNode');
  });

  afterEach(() => {
    if (modal) unmount(modal);
    modal = null;
    vi.restoreAllMocks();
  });

  /** The videos the page's own document has made, by `importNode`. */
  const madeByPage = () =>
    importNode.mock.results
      .map((result) => result.value as Node)
      .filter((node) => node instanceof HTMLVideoElement);
  const noteVideos = () => [
    ...document.querySelectorAll<HTMLVideoElement>('.markdown-content .markdown-media-video video'),
  ];

  it('shows a video element made by the page, on the first render and on a later one', async () => {
    const props = reactiveProps({
      open: true,
      title: 'Demo',
      content: note(MEDIA_ID),
      onClose: () => {},
      hashtagItems: [] as HashtagItem[],
    });
    const target = document.createElement('div');
    document.body.appendChild(target);
    modal = mount(NoteModal, { target, props });
    await settle();

    const first = noteVideos()[0];
    expect(first.getAttribute('src')).toBe(`/api/media/${MEDIA_ID}.mp4#t=0.001`);
    expectSame(noteVideos(), [first]);
    expectSame(madeByPage(), [first]);

    // The note rendered again to the same markup: the element stays, with
    // whatever the reader has done to it.
    props.hashtagItems = [];
    await settle();
    expectSame(noteVideos(), [first]);
    expectSame(madeByPage(), [first]);

    // The note was rewritten: `{@html}` builds new nodes, and the new video
    // is the page's too.
    props.content = note(OTHER_ID);
    await settle();
    const second = noteVideos()[0];
    expect(second).not.toBe(first);
    expect(second.getAttribute('src')).toBe(`/api/media/${OTHER_ID}.mp4#t=0.001`);
    expectSame(noteVideos(), [second]);
    expectSame(madeByPage(), [first, second]);
  });
});
