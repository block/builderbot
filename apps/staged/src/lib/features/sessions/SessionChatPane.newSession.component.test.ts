import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session } from '../../types';
import type { ComposerNewSessionDraft, ComposerNewSessionKind } from './composerSendControl';

/**
 * The composer's "New note" / "New commit": the pane hands its draft to the
 * host to start a new session with, instead of sending it into its own
 * session. The draft leaves the box and the store like a sent one, and comes
 * back when the host reports the start failed.
 */

const SESSION_ID = 'note-session';

const { writer, loadDraftMock, resumeSessionMock } = vi.hoisted(() => ({
  writer: {
    scheduleSave: vi.fn(),
    saveNow: vi.fn(async () => {}),
    clearNow: vi.fn(async () => {}),
    flush: vi.fn(async () => {}),
  },
  loadDraftMock: vi.fn(async (_key: string): Promise<string | null> => null),
  resumeSessionMock: vi.fn(),
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
vi.mock('../settings/preferences.svelte', () => ({ setAcpConfigPref: vi.fn() }));
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  listenToEvent: () => () => {},
}));
vi.mock('../../shared/draftCache', () => ({
  createDraftWriteScheduler: () => writer,
  loadDraft: (key: string) => loadDraftMock(key),
}));
vi.mock('../../commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../commands')>()),
  getSession: async (id: string) =>
    ({ id, status: 'completed', provider: null, workingDir: null }) as unknown as Session,
  getLatestSessionMessages: async () => ({ data: [] }),
  getSessionMessages: async () => ({ data: [] }),
  getFreshSessionMessages: async () => [],
  getSessionMessagesSince: async () => [],
  getSessionAcpMetadataMessages: async () => [],
  getSessionAcpMetadataMessagesSince: async () => [],
  listQueuedSessionMessages: async () => [],
  resumeSession: resumeSessionMock,
}));

import SessionChatPane from './SessionChatPane.svelte';
import { reactiveProps } from './testStubs/reactiveProps.svelte';

type Starter = (kind: ComposerNewSessionKind, draft: ComposerNewSessionDraft) => Promise<boolean>;

const DRAFT_KEY = `draft:session-reply:${SESSION_ID}`;

let pane: ReturnType<typeof mount> | null = null;

async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

async function mountPane(extra: { onStartNewSession?: Starter; newSessionDisabled?: boolean }) {
  const props = reactiveProps({
    active: true,
    sessionId: SESSION_ID,
    hashtagItems: [],
    newSessionDisabled: false,
    ...extra,
  });
  const target = document.createElement('div');
  document.body.appendChild(target);
  pane = mount(SessionChatPane, { target, props });
  flushSync();
  await settle();
  return props;
}

const composerInput = () => {
  const input = document.querySelector<HTMLElement>('[data-testid="prompt-input"]');
  if (!input) throw new Error('composer input not rendered');
  return input;
};

function typeInComposer(text: string) {
  const input = composerInput();
  input.textContent = text;
  input.dispatchEvent(new InputEvent('input', { bubbles: true }));
  flushSync();
}

const newSessionButton = (kind: ComposerNewSessionKind) =>
  document.querySelector<HTMLButtonElement>(`[data-testid="composer-new-${kind}"]`);

function choose(kind: ComposerNewSessionKind) {
  const button = newSessionButton(kind);
  if (!button) throw new Error(`no "${kind}" action rendered`);
  button.click();
  flushSync();
}

/** A starter whose outcome the test settles by hand. */
function deferredStarter() {
  let settleStart!: (started: boolean) => void;
  const starter = vi.fn<Starter>(
    () =>
      new Promise<boolean>((resolve) => {
        settleStart = resolve;
      })
  );
  return { starter, settleStart: (started: boolean) => settleStart(started) };
}

describe('SessionChatPane new session from the draft', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    for (const fn of Object.values(writer)) fn.mockClear();
    loadDraftMock.mockReset();
    loadDraftMock.mockImplementation(async () => null);
    resumeSessionMock.mockReset();
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    if (pane) unmount(pane);
    pane = null;
  });

  it('does not offer the actions unless the host can start a session', async () => {
    await mountPane({});
    expect(newSessionButton('note')).toBeNull();
    expect(newSessionButton('commit')).toBeNull();
  });

  it('hands the trimmed draft to the host and clears it, without sending to its own session', async () => {
    const starter = vi.fn<Starter>(async () => true);
    await mountPane({ onStartNewSession: starter });
    typeInComposer('  Turn this into a plan\n');

    choose('note');
    await settle();

    expect(starter).toHaveBeenCalledTimes(1);
    expect(starter).toHaveBeenCalledWith('note', { text: 'Turn this into a plan', imageIds: [] });
    expect(resumeSessionMock).not.toHaveBeenCalled();
    // Cleared the way a sent message is: out of the store and out of the box.
    expect(writer.clearNow).toHaveBeenCalledWith(DRAFT_KEY);
    expect(writer.saveNow).not.toHaveBeenCalled();
    typeInComposer('next');
    await settle();
    expect(writer.scheduleSave).toHaveBeenLastCalledWith(DRAFT_KEY, 'next');
  });

  it('passes the chosen kind through', async () => {
    const starter = vi.fn<Starter>(async () => true);
    await mountPane({ onStartNewSession: starter });
    typeInComposer('Implement it');

    choose('commit');
    await settle();

    expect(starter).toHaveBeenCalledWith('commit', { text: 'Implement it', imageIds: [] });
  });

  it('takes the draft out of the store before the host is called, so a parked pane holds none', async () => {
    // The host closes the dialog inside the call; whatever the pane still
    // held then would be parked with it.
    let clearedWhenCalled = false;
    const starter = vi.fn<Starter>(async () => {
      clearedWhenCalled = writer.clearNow.mock.calls.length > 0;
      return true;
    });
    await mountPane({ onStartNewSession: starter });
    typeInComposer('Draft');

    choose('note');
    await settle();

    expect(starter).toHaveBeenCalledTimes(1);
    expect(clearedWhenCalled).toBe(true);
  });

  it('ignores an empty draft, as Send does', async () => {
    const starter = vi.fn<Starter>(async () => true);
    await mountPane({ onStartNewSession: starter });
    typeInComposer('   ');

    choose('note');
    await settle();

    expect(starter).not.toHaveBeenCalled();
    expect(writer.clearNow).not.toHaveBeenCalled();
  });

  it('does nothing while the host cannot start a session', async () => {
    const starter = vi.fn<Starter>(async () => true);
    await mountPane({ onStartNewSession: starter, newSessionDisabled: true });
    typeInComposer('Draft');
    expect(newSessionButton('note')?.disabled).toBe(true);

    // A click that slipped past the disabled control is still refused.
    newSessionButton('note')!.disabled = false;
    choose('note');
    await settle();

    expect(starter).not.toHaveBeenCalled();
    expect(writer.clearNow).not.toHaveBeenCalled();
  });

  it('starts once when chosen again while the first start is in flight', async () => {
    const { starter, settleStart } = deferredStarter();
    await mountPane({ onStartNewSession: starter });
    typeInComposer('Draft');

    choose('note');
    await settle();
    typeInComposer('typed meanwhile');
    choose('commit');
    await settle();

    expect(starter).toHaveBeenCalledTimes(1);
    settleStart(true);
    await settle();
  });

  it('puts the draft back, in the box and in the store, when the start failed', async () => {
    const { starter, settleStart } = deferredStarter();
    await mountPane({ onStartNewSession: starter });
    typeInComposer('Keep me');

    choose('note');
    await settle();
    writer.scheduleSave.mockClear();
    settleStart(false);
    await settle();

    // Written straight to the store: a parked pane's autosave does not run,
    // and the next open reloads the box from there.
    expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me');
    // And back in the box for a pane that is still (or again) on screen.
    expect(writer.scheduleSave).toHaveBeenLastCalledWith(DRAFT_KEY, 'Keep me');
  });

  it('restores the draft to the store when the pane was parked meanwhile', async () => {
    const { starter, settleStart } = deferredStarter();
    const props = await mountPane({ onStartNewSession: starter });
    typeInComposer('Keep me');

    choose('note');
    await settle();
    // The host closed the dialog as it started the session.
    props.active = false;
    flushSync();
    writer.scheduleSave.mockClear();
    settleStart(false);
    await settle();

    expect(writer.scheduleSave).not.toHaveBeenCalled();
    expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me');
  });

  it('reports a starter that threw and keeps the draft', async () => {
    const starter = vi.fn<Starter>(async () => {
      throw new Error('boom');
    });
    await mountPane({ onStartNewSession: starter });
    typeInComposer('Keep me');

    choose('commit');
    await settle();

    expect(document.querySelector('[data-testid="composer-send-error"]')?.textContent).toBe(
      'Failed to start: boom'
    );
    expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me');
  });
});
