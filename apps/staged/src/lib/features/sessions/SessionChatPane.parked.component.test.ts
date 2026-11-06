import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session, SessionMessage } from '../../types';

/**
 * A pane parked by `sessionPanePool` and reopened. `park()` clears the search
 * highlights, which must leave the empty text nodes Svelte anchors its blocks
 * on alone: clearing used to normalize the transcript, deleting them, so a
 * block whose branch changed in between inserted its new content before a
 * detached anchor and rendered nothing on reopen.
 */

const SESSION_ID = 'parked-session';

const { transcript } = vi.hoisted(() => ({ transcript: { current: [] as SessionMessage[] } }));

// The tool cards and headers are real: the anchors under test live in them
// and in the transcript's tool groups.
vi.mock('./ChatComposer.svelte', async () => {
  const { default: PromptInputStub } = await import('./testStubs/PromptInputStub.svelte');
  return { default: PromptInputStub, composerControlClass: '' };
});
vi.mock('../agents/AcpFixedConfigPicker.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./PlanCard.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./PipelineSteps.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock('./NoteActivityCard.svelte', () => import('./testStubs/EmptyStub.svelte'));
vi.mock(
  '../../shared/markdown/DiagramViewerModal.svelte',
  () => import('./testStubs/EmptyStub.svelte')
);
vi.mock('../branches/dragDrop', () => ({ subscribeDragDrop: () => () => {} }));
vi.mock('../agents/agent.svelte', () => ({ agentState: { providers: [] } }));
vi.mock('../settings/preferences.svelte', () => ({ setAcpConfigPref: vi.fn() }));
vi.mock('../../shared/draftCache', () => ({
  createDraftWriteScheduler: () => ({
    scheduleSave: vi.fn(),
    saveNow: vi.fn(async () => {}),
    clearNow: vi.fn(async () => {}),
    flush: vi.fn(async () => {}),
  }),
  loadDraft: async () => null,
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
    // A reopen of the session the pane already shows fetches one delta, so the
    // transcript it kept is updated in place rather than rebuilt.
    pollSession: async (id: string, request: { sinceId: number }) => ({
      session: session(id),
      queuedMessages: [],
      messages: transcript.current.filter((m) => m.id >= request.sinceId),
      acpMetadataMessages: [],
    }),
  };
});

import SessionChatPane from './SessionChatPane.svelte';
import { reactiveProps } from './testStubs/reactiveProps.svelte';

function row(id: number, role: SessionMessage['role'], rest: Partial<SessionMessage>) {
  return { id, sessionId: SESSION_ID, role, createdAt: id, ...rest } as SessionMessage;
}

/** A completed `Read` of `path`: the call at `id` and its result at `id + 1`. */
function completedRead(id: number, path: string): SessionMessage[] {
  const toolCallId = `tc-${id}`;
  return [
    row(id, 'tool_call', {
      content: JSON.stringify({ name: 'Read', input: { file_path: path } }),
      acpEventKind: 'tool_call',
      acpToolCallId: toolCallId,
    }),
    row(id + 1, 'tool_result', { content: 'ok', acpToolCallId: toolCallId }),
  ];
}

async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

const toolHeaders = () =>
  [...document.querySelectorAll('.tool-group .tool-header')].map((el) =>
    el.textContent?.replace(/\s+/g, ' ').trim()
  );

describe('SessionChatPane parked and reopened', () => {
  let pane: (ReturnType<typeof mount> & { park(): void; unpark(): void }) | null = null;

  beforeEach(() => {
    document.body.innerHTML = '';
    vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      cb(0);
      return 0;
    });
    // Rows the reopen appends slide in; jsdom has no Web Animations.
    Element.prototype.animate = () =>
      ({ onfinish: null, cancel() {}, playState: 'finished' }) as unknown as Animation;
  });

  afterEach(() => {
    if (pane) unmount(pane);
    pane = null;
    delete (Element.prototype as Partial<Element>).animate;
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('renders a tool group that grew from one call to several while the pane was parked', async () => {
    transcript.current = [
      row(1, 'user', { content: 'Look around' }),
      ...completedRead(2, '/repo/a.ts'),
    ];
    const props = reactiveProps({ active: true, sessionId: SESSION_ID, hashtagItems: [] });
    const target = document.createElement('div');
    document.body.appendChild(target);
    pane = mount(SessionChatPane, { target, props }) as NonNullable<typeof pane>;
    await settle();
    // A lone Read is a card showing its own call.
    expect(toolHeaders()).toEqual([expect.stringMatching(/Read\s*\/repo\/a\.ts/)]);

    // What the pool does when the dialog closes.
    pane.park();
    props.active = false;
    flushSync();

    // Meanwhile the agent read two more files: the card's block becomes a
    // "Read 3 files" group header.
    transcript.current = [
      ...transcript.current,
      ...completedRead(4, '/repo/b.ts'),
      ...completedRead(6, '/repo/c.ts'),
    ];

    pane.unpark();
    props.active = true;
    flushSync();
    await settle();

    expect(toolHeaders()).toEqual([expect.stringMatching(/Read\s*3 files/)]);
  });
});
