import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest';
import type { Session, SessionMessage } from '../../types';

/**
 * A video in an assistant message. The transcript renders markdown with
 * `{@html}`, which creates the `<video>` in a template's document; WebKit
 * plays such a video with a blank picture, so the pane swaps it for one the
 * page's document made (shared/markdown/pageVideos.ts). Which document made
 * the element is all jsdom can show; the picture is checked in the real app.
 */

const SESSION_ID = 'video-session';
const MEDIA_ID = '3f2a9c1e-1234-4567-8123-123456789abc';

const { transcript } = vi.hoisted(() => ({ transcript: { current: [] as SessionMessage[] } }));

vi.mock('./ChatComposer.svelte', async () => {
  const { default: PromptInputStub } = await import('./testStubs/PromptInputStub.svelte');
  return { default: PromptInputStub, composerControlClass: '' };
});
vi.mock('../agents/AcpFixedConfigPicker.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./PlanCard.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./PipelineSteps.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./tool-calls/ToolCallCard.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./tool-calls/ToolCallHeader.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./NoteActivityCard.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock(
  '../../shared/markdown/DiagramViewerModal.svelte',
  () => import('./testStubs/EmptyStub.svelte')
);
vi.mock('../branches/dragDrop', () => ({ subscribeDragDrop: () => () => {} }));
vi.mock('../agents/agent.svelte', () => ({ agentState: { providers: [] } }));
vi.mock('../settings/preferences.svelte', () => ({
  // Read by the send control the pane hands the composer stub.
  preferences: { composerSendMode: 'queue' },
  setAcpConfigPref: vi.fn(),
}));
vi.mock('../../shared/draftCache', () => ({
  createDraftWriteScheduler: () => ({
    scheduleSave: vi.fn(),
    saveNow: vi.fn(async () => {}),
    clearNow: vi.fn(async () => {}),
    flush: vi.fn(async () => {}),
  }),
  loadDraft: async () => null,
  loadDraftContent: async () => null,
}));
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  listenToEvent: () => () => {},
}));
vi.mock('../../commands', async (importOriginal) => {
  const session = (id: string) =>
    ({ id, status: 'completed', provider: null, workingDir: null }) as unknown as Session;
  return {
    ...(await importOriginal<typeof import('../../commands')>()),
    getSession: async (id: string) => session(id),
    getLatestSessionMessages: async () => ({ data: transcript.current }),
    getSessionMessages: async () => ({ data: [] }),
    getSessionMessagesSince: async () => [],
    getSessionAcpMetadataMessages: async () => [],
    getSessionAcpMetadataMessagesSince: async () => [],
    listQueuedSessionMessages: async () => [],
  };
});

import SessionChatPane from './SessionChatPane.svelte';
import { reactiveProps } from './testStubs/reactiveProps.svelte';

function row(id: number, role: SessionMessage['role'], content: string) {
  return { id, sessionId: SESSION_ID, role, createdAt: id, content } as SessionMessage;
}

async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

describe('SessionChatPane video in an assistant message', () => {
  let pane: ReturnType<typeof mount> | null = null;
  let importNode: MockInstance<Document['importNode']>;

  beforeEach(() => {
    document.body.innerHTML = '';
    vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      cb(0);
      return 0;
    });
    // jsdom only reports that it has no `load`.
    vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(() => {});
    importNode = vi.spyOn(document, 'importNode');
  });

  afterEach(() => {
    if (pane) unmount(pane);
    pane = null;
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('shows a video element made by the page, not the one {@html} created', async () => {
    transcript.current = [
      row(1, 'user', 'Show me the recording'),
      row(2, 'assistant', `Here it is.\n\n![Walkthrough](staged-media://${MEDIA_ID}.mp4)`),
    ];
    const props = reactiveProps({ active: true, sessionId: SESSION_ID, hashtagItems: [] });
    const target = document.createElement('div');
    document.body.appendChild(target);
    pane = mount(SessionChatPane, { target, props });
    await settle();

    const videos = [...document.querySelectorAll('.assistant-message .markdown-media-video video')];
    expect(videos).toHaveLength(1);
    const video = videos[0] as HTMLVideoElement;
    expect(video.getAttribute('src')).toBe(`/api/media/${MEDIA_ID}.mp4#t=0.001`);
    // Svelte itself clones its templates with `cloneNode` here, so the only
    // video `importNode` has returned is the copy made for this one.
    const imported = importNode.mock.results
      .map((result) => result.value as Node)
      .filter((node) => node instanceof HTMLVideoElement);
    // The same element, not an equal one: `toEqual` compares nodes by markup.
    expect(imported).toHaveLength(1);
    expect(imported[0]).toBe(video);
    expect(video.ownerDocument).toBe(document);
  });
});
