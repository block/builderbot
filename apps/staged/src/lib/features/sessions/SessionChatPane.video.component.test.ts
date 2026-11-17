import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest';
import type { Session, SessionMessage, SessionPollRequest } from '../../types';

/**
 * A video in an assistant message. The transcript renders markdown with
 * `{@html}`, which creates the `<video>` in a template's document; WebKit
 * plays such a video with a blank picture, so the pane swaps it for one the
 * page's document made (shared/markdown/pageVideos.ts). Which document made
 * the element is all jsdom can show; the picture is checked in the real app.
 */

const SESSION_ID = 'video-session';
const MEDIA_ID = '3f2a9c1e-1234-4567-8123-123456789abc';

/** What the backend holds: the pane loads it, and polls it while `running`. */
const { transcript, sessionStatus } = vi.hoisted(() => ({
  transcript: { current: [] as SessionMessage[] },
  sessionStatus: { current: 'completed' },
}));

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
    ({ id, status: sessionStatus.current, provider: null, workingDir: null }) as unknown as Session;
  return {
    ...(await importOriginal<typeof import('../../commands')>()),
    getSession: async (id: string) => session(id),
    // One tick of a running session: the last row the pane holds, which may
    // have grown, and any after it.
    pollSession: async (id: string, request: SessionPollRequest) => ({
      session: session(id),
      queuedMessages: [],
      messages: transcript.current.filter((message) => message.id >= request.sinceId),
      acpMetadataMessages: [],
    }),
    getSessionLiveDelivery: async () => ({ supported: false, retryable: false }),
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
    sessionStatus.current = 'completed';
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
    vi.useRealTimers();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  const ANSWER = `Here it is.\n\n![Walkthrough](staged-media://${MEDIA_ID}.mp4)`;

  async function mountPane() {
    const props = reactiveProps({ active: true, sessionId: SESSION_ID, hashtagItems: [] });
    const target = document.createElement('div');
    document.body.appendChild(target);
    pane = mount(SessionChatPane, { target, props });
    await settle();
  }

  /** The one video in the assistant message. */
  function messageVideo(): HTMLVideoElement {
    const videos = [...document.querySelectorAll('.assistant-message .markdown-media-video video')];
    expect(videos).toHaveLength(1);
    return videos[0] as HTMLVideoElement;
  }

  /**
   * The videos the page's own document has made. Svelte itself clones its
   * templates with `cloneNode` here, so these are the copies and nothing else.
   */
  const madeByPage = () =>
    importNode.mock.results
      .map((result) => result.value as Node)
      .filter((node) => node instanceof HTMLVideoElement);

  it('shows a video element made by the page, not the one {@html} created', async () => {
    transcript.current = [row(1, 'user', 'Show me the recording'), row(2, 'assistant', ANSWER)];
    await mountPane();

    const video = messageVideo();
    expect(video.getAttribute('src')).toBe(`/api/media/${MEDIA_ID}.mp4#t=0.001`);
    // The same element, not an equal one: `toEqual` compares nodes by markup.
    expect(madeByPage()).toHaveLength(1);
    expect(madeByPage()[0]).toBe(video);
    expect(video.ownerDocument).toBe(document);
  });

  it('shows one again when the message is rendered again as it streams', async () => {
    // Only the poll's interval: Svelte's own scheduling stays real.
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    sessionStatus.current = 'running';
    transcript.current = [row(1, 'user', 'Show me the recording'), row(2, 'assistant', ANSWER)];
    await mountPane();
    const first = messageVideo();
    const content = first.closest('.markdown-content');

    // The next chunk: the same row, longer. The poll brings it in, and the
    // message's `{@html}` is given the whole markup again.
    transcript.current = [transcript.current[0], row(2, 'assistant', `${ANSWER}\n\nThat is all.`)];
    vi.advanceTimersByTime(500);
    await settle();

    const second = messageVideo();
    expect(second.closest('.markdown-content')?.textContent).toContain('That is all.');
    // The same message element with new nodes in it, so the video is a new
    // one, and it has to be the page's as the first was.
    expect(second.closest('.markdown-content')).toBe(content);
    expect(second).not.toBe(first);
    expect(madeByPage()).toHaveLength(2);
    expect(madeByPage()[1]).toBe(second);
    expect(second.ownerDocument).toBe(document);
  });
});
