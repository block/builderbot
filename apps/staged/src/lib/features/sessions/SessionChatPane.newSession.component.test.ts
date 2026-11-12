import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session } from '../../types';
import type { ComposerNewSessionDraft, ComposerNewSessionKind } from './composerSendControl';

/**
 * The composer's "New note" / "New commit": the pane hands its draft to the
 * host to start a new session with, instead of sending it into its own
 * session. The draft leaves the box and the store like a sent one, and comes
 * back when the host reports the start failed: above anything typed since,
 * in the box when the pane is on screen and in the store (and the parked
 * box) when it is not, with the failure said on the composer's line and,
 * for a closed dialog, by a toast. A pane that is gone by then hands the
 * message to a pane showing the session, or leaves it in a toast to copy.
 */

const SESSION_ID = 'note-session';

const { writer, loadDraftMock, resumeSessionMock, toastMock, clipboardMock } = vi.hoisted(() => ({
  toastMock: {
    error: vi.fn((_title: string, _options?: unknown) => 'error-toast'),
    info: vi.fn((_title: string, _options?: unknown) => 'info-toast'),
    dismiss: vi.fn(),
  },
  clipboardMock: vi.fn(async (_text: string) => {}),
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
vi.mock('../settings/preferences.svelte', () => ({
  // Read by the send control the pane hands the composer stub.
  preferences: { composerSendMode: 'queue' },
  setAcpConfigPref: vi.fn(),
}));
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  listenToEvent: () => () => {},
  writeClipboardText: clipboardMock,
}));
vi.mock('svelte-sonner', () => ({ toast: toastMock }));
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

function unmountPane() {
  if (pane) unmount(pane);
  pane = null;
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

/** What the pane has put in the box (the stub mirrors the bound value). */
const boxText = () => composerInput().dataset.value;

const sendErrorLine = () =>
  document.querySelector('[data-testid="composer-send-error"]')?.textContent;

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

/** Makes every draft read from here on wait until the test settles it. */
function deferDraftLoads() {
  const settlers: Array<(draft: string | null) => void> = [];
  loadDraftMock.mockImplementation(
    () =>
      new Promise<string | null>((resolve) => {
        settlers.push(resolve);
      })
  );
  return {
    pending: () => settlers.length,
    /** Settle the oldest read still waiting. */
    settleNext: (draft: string | null) => {
      const settleLoad = settlers.shift();
      if (!settleLoad) throw new Error('no draft read is waiting');
      settleLoad(draft);
    },
  };
}

const NOT_STARTED = 'The new session did not start. Your message is back in the box.';
const NOT_STARTED_ABOVE =
  'The new session did not start. Your message is back in the box, above what you typed since.';
const TOAST_TITLE = 'The new session did not start';
const SAVED_AS_DRAFT =
  "Your message was saved as the note's chat draft. Copy it from here if the chat does not show it.";

/** The "Copy message" action of the newest toast, as sonner would call it. */
function toastCopyAction() {
  const options = toastMock.info.mock.calls.at(-1)?.[1] as
    { action?: { label: string; onClick: (event: MouseEvent) => void } } | undefined;
  if (!options?.action) throw new Error('the toast offers no action');
  return options.action;
}

describe('SessionChatPane new session from the draft', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    for (const fn of Object.values(writer)) fn.mockClear();
    loadDraftMock.mockReset();
    loadDraftMock.mockImplementation(async () => null);
    resumeSessionMock.mockReset();
    toastMock.error.mockClear();
    toastMock.info.mockClear();
    toastMock.dismiss.mockClear();
    clipboardMock.mockReset();
    clipboardMock.mockImplementation(async () => {});
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    unmountPane();
  });

  it('does not offer the actions unless the host can start a session', async () => {
    await mountPane({});
    expect(newSessionButton('note')).toBeNull();
    expect(newSessionButton('commit')).toBeNull();
  });

  it('hands the composer one control, split exactly when the host can start a session', async () => {
    // The pane derives the control it acts on with the same inputs the
    // composer draws from, so an idle session's button is split only here.
    await mountPane({});
    expect(composerInput().dataset.sendSplit).toBe('false');
    unmountPane();
    document.body.innerHTML = '';

    await mountPane({ onStartNewSession: vi.fn<Starter>(async () => true) });
    expect(composerInput().dataset.sendSplit).toBe('true');
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

  it('says nothing, by toast or on the composer, after a start that worked', async () => {
    const { starter, settleStart } = deferredStarter();
    const props = await mountPane({ onStartNewSession: starter });
    typeInComposer('Turn this into a plan');

    choose('note');
    await settle();
    // The host closed the dialog as it started the session.
    props.active = false;
    flushSync();
    settleStart(true);
    await settle();

    expect(toastMock.info).not.toHaveBeenCalled();
    expect(toastMock.error).not.toHaveBeenCalled();
    expect(writer.saveNow).not.toHaveBeenCalled();
    props.active = true;
    flushSync();
    await settle();
    expect(sendErrorLine()).toBeUndefined();
    expect(boxText()).toBe('');
    expect(toastMock.dismiss).not.toHaveBeenCalled();
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
    expect(boxText()).toBe('');
    writer.scheduleSave.mockClear();
    settleStart(false);
    await settle();

    // Written straight to the store as well as through the box's autosave.
    expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me');
    expect(boxText()).toBe('Keep me');
    expect(writer.scheduleSave).toHaveBeenLastCalledWith(DRAFT_KEY, 'Keep me');
  });

  it('says on the composer that the session did not start and the message is back', async () => {
    const starter = vi.fn<Starter>(async () => false);
    await mountPane({ onStartNewSession: starter });
    typeInComposer('Keep me');

    choose('note');
    await settle();

    expect(sendErrorLine()).toBe(NOT_STARTED);
    // The pane is on screen, so its own line says it: no toast on top.
    expect(toastMock.info).not.toHaveBeenCalled();
    expect(toastMock.error).not.toHaveBeenCalled();
  });

  it('reports a starter that threw, with its reason, and keeps the draft', async () => {
    const starter = vi.fn<Starter>(async () => {
      throw new Error('boom');
    });
    await mountPane({ onStartNewSession: starter });
    typeInComposer('Keep me');

    choose('commit');
    await settle();

    expect(sendErrorLine()).toBe(
      'The new session did not start (boom). Your message is back in the box.'
    );
    expect(boxText()).toBe('Keep me');
    expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me');
  });

  describe('when something was typed after the draft was handed over', () => {
    it('puts the message back above text typed while the start was in flight', async () => {
      const { starter, settleStart } = deferredStarter();
      await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      // The box stays editable while the start is in flight.
      typeInComposer('typed since');
      await settle();
      settleStart(false);
      await settle();

      expect(boxText()).toBe('Keep me\n\ntyped since');
      expect(writer.saveNow).toHaveBeenCalledTimes(1);
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me\n\ntyped since');
      expect(writer.scheduleSave).toHaveBeenLastCalledWith(DRAFT_KEY, 'Keep me\n\ntyped since');
      expect(sendErrorLine()).toBe(NOT_STARTED_ABOVE);
      expect(toastMock.info).not.toHaveBeenCalled();
    });

    it('puts the message back above text typed after the note was reopened', async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      // The host closed the dialog; the user opens the note again and types.
      props.active = false;
      flushSync();
      props.active = true;
      flushSync();
      await settle();
      typeInComposer('typed since');
      await settle();
      settleStart(false);
      await settle();

      expect(boxText()).toBe('Keep me\n\ntyped since');
      expect(writer.saveNow).toHaveBeenCalledTimes(1);
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me\n\ntyped since');
      expect(sendErrorLine()).toBe(NOT_STARTED_ABOVE);
    });

    it('joins in the store when the pane is parked again, after flushing its pending write', async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      // Closed by the host, reopened, typed in, and closed again: the newer
      // text is in the store (or still in the writer's debounce) by now.
      props.active = false;
      flushSync();
      loadDraftMock.mockImplementation(async () => 'typed after closing');
      writer.flush.mockClear();
      writer.scheduleSave.mockClear();
      settleStart(false);
      await settle();

      expect(writer.saveNow).toHaveBeenCalledTimes(1);
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me\n\ntyped after closing');
      // The read saw the writer's pending keystrokes, not a stale value.
      expect(writer.flush).toHaveBeenCalled();
      expect(writer.flush.mock.invocationCallOrder[0]).toBeLessThan(
        loadDraftMock.mock.invocationCallOrder.at(-1)!
      );
      expect(writer.scheduleSave).not.toHaveBeenCalled();
    });

    it('waits out a draft load still in flight, then joins in the box', async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      props.active = false;
      flushSync();
      // Reopened: the box does not yet hold what the store has for it.
      const loads = deferDraftLoads();
      props.active = true;
      flushSync();
      await settle();
      expect(loads.pending()).toBe(1);
      settleStart(false);
      await settle();
      // Neither place is written while it is unknown which holds the draft.
      expect(writer.saveNow).not.toHaveBeenCalled();
      expect(boxText()).toBe('');

      loads.settleNext('typed in an earlier open');
      await settle();

      expect(boxText()).toBe('Keep me\n\ntyped in an earlier open');
      expect(writer.saveNow).toHaveBeenCalledTimes(1);
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me\n\ntyped in an earlier open');
      expect(sendErrorLine()).toBe(NOT_STARTED_ABOVE);
      expect(toastMock.info).not.toHaveBeenCalled();
    });

    it('joins in the box when the note is reopened while the store is being read', async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      props.active = false;
      flushSync();
      const loads = deferDraftLoads();
      settleStart(false);
      await settle();
      // The parked pane is reading the store to join into it...
      expect(loads.pending()).toBe(1);
      // ...when the note is reopened, which starts the box's own load.
      props.active = true;
      flushSync();
      await settle();
      expect(loads.pending()).toBe(2);

      loads.settleNext('typed earlier');
      await settle();
      // The store is no longer the place: nothing is written to it blind.
      expect(writer.saveNow).not.toHaveBeenCalled();
      loads.settleNext('typed earlier');
      await settle();

      expect(boxText()).toBe('Keep me\n\ntyped earlier');
      expect(writer.saveNow).toHaveBeenCalledTimes(1);
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me\n\ntyped earlier');
      expect(toastMock.info).not.toHaveBeenCalled();
    });

    it('reads the store again when the note was opened and closed during the read', async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      props.active = false;
      flushSync();
      const loads = deferDraftLoads();
      settleStart(false);
      await settle();
      expect(loads.pending()).toBe(1);
      // Opened, typed in and closed again before that read came back.
      props.active = true;
      flushSync();
      await settle();
      props.active = false;
      flushSync();
      await settle();

      // The first read predates what was typed: it is not written from.
      loads.settleNext(null);
      await settle();
      expect(writer.saveNow).not.toHaveBeenCalled();
      // The open's own load, abandoned when the pane was parked again.
      loads.settleNext(null);
      await settle();
      expect(writer.saveNow).not.toHaveBeenCalled();
      loads.settleNext('typed in between');
      await settle();

      expect(writer.saveNow).toHaveBeenCalledTimes(1);
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me\n\ntyped in between');
    });

    it("falls back to the parked box's text when the store cannot be read", async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      typeInComposer('typed since');
      await settle();
      props.active = false;
      flushSync();
      loadDraftMock.mockImplementation(async () => {
        throw new Error('store unavailable');
      });
      settleStart(false);
      await settle();

      expect(writer.saveNow).toHaveBeenCalledTimes(1);
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me\n\ntyped since');
    });
  });

  describe('when the dialog is closed by the time the start fails', () => {
    it('restores the draft to the store and to the parked box, without an autosave', async () => {
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

      // A parked pane's autosave does not run, so the store is written
      // directly; the box is given the same text, so the two agree before
      // the next open reloads it from the store.
      expect(writer.scheduleSave).not.toHaveBeenCalled();
      expect(writer.saveNow).toHaveBeenCalledTimes(1);
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me');
      expect(boxText()).toBe('Keep me');
    });

    it('gives the parked box the joined text when the store held newer text', async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      props.active = false;
      flushSync();
      loadDraftMock.mockImplementation(async () => 'typed after closing');
      settleStart(false);
      await settle();

      expect(boxText()).toBe('Keep me\n\ntyped after closing');
    });

    it('still holds the message when the reopen cannot read the store', async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      props.active = false;
      flushSync();
      settleStart(false);
      await settle();
      loadDraftMock.mockImplementation(async () => {
        throw new Error('store unavailable');
      });
      writer.scheduleSave.mockClear();
      props.active = true;
      flushSync();
      await settle();

      // The failed load leaves the box as it was, which is the message, so
      // the line beside it is true and the next keystroke saves both.
      expect(boxText()).toBe('Keep me');
      expect(sendErrorLine()).toBe(NOT_STARTED);
      expect(writer.scheduleSave).not.toHaveBeenCalled();
      typeInComposer('Keep me, and more');
      await settle();
      expect(writer.scheduleSave).toHaveBeenLastCalledWith(DRAFT_KEY, 'Keep me, and more');
    });

    it("toasts that the session did not start and the message is back in the note's chat", async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      props.active = false;
      flushSync();
      settleStart(false);
      await settle();

      // The host's launch helper has already toasted why; this one says
      // where the message went, and stays until dismissed or the note's
      // chat is opened again.
      expect(toastMock.error).not.toHaveBeenCalled();
      expect(toastMock.info).toHaveBeenCalledTimes(1);
      expect(toastMock.info).toHaveBeenCalledWith(TOAST_TITLE, {
        description: "Your message is back in the note's chat box.",
        duration: Infinity,
      });
    });

    it('says in the toast when the message went back above newer text', async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      props.active = false;
      flushSync();
      loadDraftMock.mockImplementation(async () => 'typed after closing');
      settleStart(false);
      await settle();

      expect(toastMock.info).toHaveBeenCalledWith(TOAST_TITLE, {
        description: "Your message is back in the note's chat box, above what you typed since.",
        duration: Infinity,
      });
    });

    it('toasts a starter that threw as an error, with its reason', async () => {
      let failStart!: (error: Error) => void;
      const starter = vi.fn<Starter>(
        () =>
          new Promise<boolean>((_resolve, reject) => {
            failStart = reject;
          })
      );
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('commit');
      await settle();
      props.active = false;
      flushSync();
      failStart(new Error('boom'));
      await settle();

      expect(toastMock.info).not.toHaveBeenCalled();
      expect(toastMock.error).toHaveBeenCalledTimes(1);
      expect(toastMock.error).toHaveBeenCalledWith(TOAST_TITLE, {
        description: "boom. Your message is back in the note's chat box.",
        duration: Infinity,
      });
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me');
    });

    it('still shows the failure beside the restored message when the note is reopened', async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      props.active = false;
      flushSync();
      settleStart(false);
      await settle();
      // The store now holds what was written back.
      const restored = writer.saveNow.mock.calls.at(-1) as unknown as [string, string];
      loadDraftMock.mockImplementation(async () => restored[1]);
      props.active = true;
      flushSync();
      await settle();

      expect(boxText()).toBe('Keep me');
      expect(sendErrorLine()).toBe(NOT_STARTED);
    });

    it("takes the toast down when the note's chat is opened again", async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      props.active = false;
      flushSync();
      settleStart(false);
      await settle();
      // Still up while the note stays closed: nothing else says it yet.
      expect(toastMock.info).toHaveBeenCalledTimes(1);
      expect(toastMock.dismiss).not.toHaveBeenCalled();

      props.active = true;
      flushSync();
      await settle();
      // The composer's line says it from here.
      expect(toastMock.dismiss).toHaveBeenCalledTimes(1);
      expect(toastMock.dismiss).toHaveBeenCalledWith('info-toast');
      expect(sendErrorLine()).toBe(NOT_STARTED);

      // Once: a later open has no toast left to take down.
      props.active = false;
      flushSync();
      props.active = true;
      flushSync();
      await settle();
      expect(toastMock.dismiss).toHaveBeenCalledTimes(1);
    });
  });

  describe('when the pane is gone by the time the start fails', () => {
    it('hands the message to another pane that has the session on screen', async () => {
      const { starter, settleStart } = deferredStarter();
      await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      // The pane that took the message is unmounted (a transient pane, or
      // one evicted from the pool), and the chat is open in a new one.
      unmountPane();
      await mountPane({});
      typeInComposer('typed in the other pane');
      await settle();
      writer.saveNow.mockClear();
      settleStart(false);
      await settle();

      // Joined in the box that is on screen, never written to the store
      // behind it, where its next keystroke would have saved over it.
      expect(boxText()).toBe('Keep me\n\ntyped in the other pane');
      expect(writer.saveNow).toHaveBeenCalledTimes(1);
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me\n\ntyped in the other pane');
      expect(sendErrorLine()).toBe(NOT_STARTED_ABOVE);
      expect(toastMock.info).not.toHaveBeenCalled();
      expect(toastMock.error).not.toHaveBeenCalled();
    });

    it('hands it over when the other pane opens while the store is being read', async () => {
      const { starter, settleStart } = deferredStarter();
      await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      unmountPane();
      const loads = deferDraftLoads();
      settleStart(false);
      await settle();
      // Nothing shows the session, so the store is read to join into it...
      expect(loads.pending()).toBe(1);
      // ...when the chat is opened in a new pane, which loads its own draft.
      await mountPane({});
      expect(loads.pending()).toBe(2);

      loads.settleNext('typed earlier');
      await settle();
      // The store is no longer the place: nothing is written to it blind.
      expect(writer.saveNow).not.toHaveBeenCalled();
      loads.settleNext('typed earlier');
      await settle();

      expect(boxText()).toBe('Keep me\n\ntyped earlier');
      expect(writer.saveNow).toHaveBeenCalledTimes(1);
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me\n\ntyped earlier');
      expect(sendErrorLine()).toBe(NOT_STARTED_ABOVE);
      expect(toastMock.info).not.toHaveBeenCalled();
    });

    it('saves the draft and says so, without claiming a box, when nothing shows the session', async () => {
      const { starter, settleStart } = deferredStarter();
      await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      unmountPane();
      writer.saveNow.mockClear();
      settleStart(false);
      await settle();

      expect(writer.saveNow).toHaveBeenCalledTimes(1);
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me');
      expect(toastMock.info).toHaveBeenCalledTimes(1);
      expect(toastMock.info).toHaveBeenCalledWith(TOAST_TITLE, {
        description: SAVED_AS_DRAFT,
        duration: Infinity,
        action: expect.objectContaining({ label: 'Copy message' }),
      });
    });

    it('copies the message from the toast, which stays up until the copy worked', async () => {
      const { starter, settleStart } = deferredStarter();
      await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      unmountPane();
      settleStart(false);
      await settle();
      const copy = toastCopyAction();

      // A copy that fails leaves the toast: it still holds the message.
      clipboardMock.mockImplementationOnce(async () => {
        throw new Error('clipboard unavailable');
      });
      const failedPress = { preventDefault: vi.fn() };
      copy.onClick(failedPress as unknown as MouseEvent);
      await settle();
      // sonner closes a toast on its action unless the press is cancelled.
      expect(failedPress.preventDefault).toHaveBeenCalled();
      expect(toastMock.dismiss).not.toHaveBeenCalled();

      copy.onClick({ preventDefault: vi.fn() } as unknown as MouseEvent);
      await settle();
      expect(clipboardMock).toHaveBeenLastCalledWith('Keep me');
      expect(toastMock.dismiss).toHaveBeenCalledTimes(1);
      expect(toastMock.dismiss).toHaveBeenCalledWith('info-toast');
    });

    it('treats a pane that moved to another session the same way', async () => {
      const { starter, settleStart } = deferredStarter();
      const props = await mountPane({ onStartNewSession: starter });
      typeInComposer('Keep me');

      choose('note');
      await settle();
      props.sessionId = 'another-session';
      flushSync();
      await settle();
      writer.saveNow.mockClear();
      settleStart(false);
      await settle();

      // Saved under the session it was written for; the box and the line now
      // belong to the other session and are left alone.
      expect(writer.saveNow).toHaveBeenCalledTimes(1);
      expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, 'Keep me');
      expect(boxText()).toBe('');
      expect(sendErrorLine()).toBeUndefined();
      expect(toastMock.info).toHaveBeenCalledWith(TOAST_TITLE, {
        description: SAVED_AS_DRAFT,
        duration: Infinity,
        action: expect.objectContaining({ label: 'Copy message' }),
      });
      // Opening that other session's chat is not opening the note's.
      props.active = false;
      flushSync();
      props.active = true;
      flushSync();
      await settle();
      expect(toastMock.dismiss).not.toHaveBeenCalled();
    });
  });
});
