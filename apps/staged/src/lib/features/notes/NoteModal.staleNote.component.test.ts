import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * Regression: NoteModal fell back to the stale snapshot it was opened with.
 *
 * The hosts (BranchCard, ProjectSection, DiffModal, ReferenceModalHost) keep
 * the open note as a snapshot object and, on every chat/note toggle,
 * REASSIGN it (`openNote = { ...openNote, chatOpen }`). The modal used to
 * take props arriving as "fresh props" and drop the note it had fetched
 * since — the only place the refreshed note lived. It also never fetched on
 * open, so a snapshot already behind the store was corrected only by
 * accident (marking an unread note read publishes `notes-changed`).
 */

const h = vi.hoisted(() => ({
  listeners: new Map<string, Set<(payload: unknown) => void>>(),
  invoke: null as null | ((command: string, args?: Record<string, unknown>) => unknown),
  /** Wide window (note and chat side by side) vs. one pane at a time. */
  canSplit: true,
  /**
   * While set, listeners are queued here instead of going live, as in the
   * desktop app, where registration is a round trip: events emitted
   * meanwhile reach nobody.
   */
  pendingListeners: null as null | (() => void)[],
}));

vi.mock('$lib/components/ui/dialog', async () => {
  const { default: Passthrough } = await import('../sessions/testStubs/PassthroughStub.svelte');
  const { default: Empty } = await import('../sessions/testStubs/EmptyStub.svelte');
  const passthrough = new Set(['Root', 'Content', 'Header', 'Title']);
  const isComponentName = (prop: PropertyKey): prop is string =>
    typeof prop === 'string' && /^[A-Z]/.test(prop);
  return new Proxy({} as Record<string, unknown>, {
    has: (_t, prop) => isComponentName(prop),
    get: (_t, prop) =>
      isComponentName(prop) ? (passthrough.has(prop) ? Passthrough : Empty) : undefined,
  });
});
vi.mock(
  '../sessions/PooledSessionChatPane.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock(
  './NoteTransferMenu.svelte',
  () => import('../sessions/testStubs/NoteTransferMenuStub.svelte')
);
vi.mock('./NoteReplyQuotes.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock(
  '../../shared/markdown/DiagramViewerModal.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock(
  '../../shared/markdown/MediaViewerModal.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('../../shared/persistentStore', () => ({
  getStoreValue: vi.fn(async () => undefined),
  setStoreValue: vi.fn(async () => {}),
  deleteStoreValue: vi.fn(async () => {}),
  listStoreKeys: vi.fn(async () => []),
  listenToStoreChanges: vi.fn(() => () => {}),
}));
vi.mock('../../shared/viewport.svelte', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../shared/viewport.svelte')>()),
  viewport: {
    isMobile: false,
    showShortcutHints: true,
    hasCoarsePointer: false,
    sheetMenus: false,
    shortWindow: false,
    get canSplit() {
      return h.canSplit;
    },
  },
}));
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  invokeCommand: async (command: string, args?: Record<string, unknown>) =>
    h.invoke!(command, args),
  listenToEvent: (
    event: string,
    callback: (payload: unknown) => void,
    opts?: { onEstablished?: () => void }
  ) => {
    let cancelled = false;
    const goLive = () => {
      if (cancelled) return;
      const set = h.listeners.get(event) ?? new Set();
      h.listeners.set(event, set);
      set.add(callback);
      // Asynchronous, as the real transport promises.
      queueMicrotask(() => {
        if (!cancelled) opts?.onEstablished?.();
      });
    };
    if (h.pendingListeners) h.pendingListeners.push(goLive);
    else goLive();
    return () => {
      cancelled = true;
      h.listeners.get(event)?.delete(callback);
    };
  },
}));

import NoteModal from './NoteModal.svelte';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

async function settle() {
  for (let i = 0; i < 12; i++) await tick();
  flushSync();
}

function emit(event: string, payload: unknown) {
  for (const callback of h.listeners.get(event) ?? []) callback(payload);
}

interface OpenNoteState {
  noteId?: string;
  title: string;
  content: string;
  sessionId?: string;
  /** Hosts derive this live rather than snapshot it; a test sets it by hand. */
  sessionStatus?: string;
  noteUpdatedAt?: number;
  chatOpen?: boolean;
}

describe('NoteModal stale note', () => {
  type Row = {
    id: string;
    sessionId: string | null;
    title: string;
    content: string;
    updatedAt: number;
    /** Null until the note's content lands. */
    completedAt: number | null;
    readAt: number | null;
  };
  let modal: ReturnType<typeof mount> | null = null;
  let root: HTMLElement;
  /**
   * The note rows as the store holds them, by id. The commands below look a
   * row up by the id or session they are asked for, as the store does, so a
   * fetch for one note cannot answer with another's row.
   */
  let rows: Map<string, Row>;
  const put = (row: Row) => rows.set(row.id, row);
  let assistantMessagesAfterNote = 0;
  /**
   * While set, `get_branch_note_by_session` answers only once it resolves.
   * Each request waits on the hold in place when it was made, so two fetches
   * can be held apart and released in either order.
   */
  let holdFetch: Promise<void> | null = null;
  /** Every command the modal invoked, in order, with its arguments. */
  let invoked: { command: string; args: Record<string, unknown> }[];
  const calls = (command: string) =>
    invoked.filter((call) => call.command === command).map((call) => call.args);

  const V1: Row = {
    id: 'note-1',
    sessionId: 'session-1',
    title: 'Plan',
    content: 'OLD body v1',
    updatedAt: 1_000,
    completedAt: 1_000,
    readAt: 1_500,
  };
  const V2: Row = {
    id: 'note-1',
    sessionId: 'session-1',
    title: 'Plan',
    content: 'NEW body v2',
    updatedAt: 2_000,
    completedAt: 1_000,
    readAt: null,
  };
  /**
   * The row a note session starts with, before it has written anything:
   * `Note::new(branch_id, prompt, "")` in session_commands.rs. Titled with
   * the prompt, empty, and not landed.
   */
  const PLACEHOLDER: Row = {
    id: 'note-1',
    sessionId: 'session-1',
    title: 'Write up the migration plan',
    content: '',
    updatedAt: 100,
    completedAt: null,
    readAt: null,
  };
  /** The runner's first save over the placeholder. */
  const FIRST_SAVE: Row = {
    ...PLACEHOLDER,
    title: 'Plan',
    content: 'First body',
    updatedAt: 2_000,
    completedAt: 2_000,
  };
  /** Another note, behind another session, for reference navigation to swap in. */
  const OTHER: Row = {
    id: 'note-2',
    sessionId: 'session-2',
    title: 'Other',
    content: 'OTHER body v2',
    updatedAt: 900,
    completedAt: 700,
    readAt: 950,
  };
  const SNAPSHOT_V1: OpenNoteState = {
    noteId: 'note-1',
    title: 'Plan',
    content: 'OLD body v1',
    sessionId: 'session-1',
    noteUpdatedAt: 1_000,
  };
  /** `OTHER` as a reference-history entry pushed before its last write. */
  const SNAPSHOT_OTHER: OpenNoteState = {
    noteId: 'note-2',
    title: 'Other',
    content: 'OTHER body v1',
    sessionId: 'session-2',
    noteUpdatedAt: 800,
  };
  /** BranchCard.openNoteChatForSession's pending-note branch: no id, chat first. */
  const SNAPSHOT_PENDING: OpenNoteState = {
    title: 'Generating note',
    content: '',
    sessionId: 'session-1',
    sessionStatus: 'running',
    chatOpen: true,
  };

  function hold(): () => void {
    let release!: () => void;
    holdFetch = new Promise<void>((resolve) => (release = resolve));
    return release;
  }

  beforeEach(() => {
    document.body.innerHTML = '';
    root = document.createElement('div');
    document.body.appendChild(root);
    h.listeners.clear();
    rows = new Map();
    assistantMessagesAfterNote = 0;
    holdFetch = null;
    invoked = [];
    h.canSplit = true;
    h.pendingListeners = null;
    h.invoke = async (command, args = {}) => {
      invoked.push({ command, args });
      // The lookup by id, for a note that has no session.
      if (command === 'get_note') {
        const row = rows.get(args.noteId as string);
        return row ? { ...row } : null;
      }
      if (command === 'get_branch_note_by_session') {
        // Read at request time, as the store would, then held like a slow reply.
        const row = [...rows.values()].find((r) => r.sessionId === args.sessionId);
        const reply = row ? { ...row } : null;
        const held = holdFetch;
        if (held) await held;
        return reply;
      }
      if (command === 'count_assistant_messages_after') return assistantMessagesAfterNote;
      if (command === 'get_media_dimensions') return {};
      if (command === 'set_note_read') {
        const row = rows.get(args.noteId as string);
        if (!row) throw new Error(`Note not found: ${String(args.noteId)}`);
        // store/notes.rs `write_read_at`: only a note that has landed can be
        // marked read, and `Notes` is published only when the write changed
        // something.
        const changed = args.read
          ? row.completedAt !== null && (row.readAt === null || row.readAt < row.completedAt)
          : row.readAt !== null;
        const next = changed
          ? { ...row, readAt: args.read ? Math.max(9_999, row.completedAt ?? 0) : null }
          : row;
        rows.set(next.id, next);
        if (changed)
          queueMicrotask(() => emit('notes-changed', { branchId: 'branch-1', projectId: null }));
        return { ...next, branchId: 'branch-1', parentProjectNoteId: null };
      }
      return null;
    };
  });

  afterEach(() => {
    if (modal) unmount(modal);
    modal = null;
  });

  /** Mount the modal the way BranchCard.svelte does. */
  async function openFromHost(initial: OpenNoteState) {
    const host = reactiveProps<{ openNote: OpenNoteState }>({ openNote: initial });
    modal = mount(NoteModal, {
      target: root,
      props: {
        open: true,
        get title() {
          return host.openNote.title;
        },
        get content() {
          return host.openNote.content;
        },
        get sessionId() {
          return host.openNote.sessionId;
        },
        get sessionStatus() {
          return host.openNote.sessionStatus;
        },
        get noteUpdatedAt() {
          return host.openNote.noteUpdatedAt;
        },
        get noteId() {
          return host.openNote.noteId;
        },
        noteKind: 'branch' as const,
        branchId: 'branch-1',
        projectId: 'project-1',
        get chatOpen() {
          return host.openNote.chatOpen ?? false;
        },
        // BranchCard's handler, verbatim.
        onChatOpenChange: (chatOpen: boolean) => {
          if (host.openNote) host.openNote = { ...host.openNote, chatOpen };
        },
        onClose: () => {},
      },
    });
    await settle();
    return host;
  }

  const noteText = () => root.querySelector('.modal-content')?.textContent?.trim() ?? '';
  const transferMenu = () => root.querySelector<HTMLElement>('[data-testid="note-transfer-menu"]');
  const chatToggle = () =>
    [...root.querySelectorAll<HTMLButtonElement>('button')].find((b) =>
      /chat pane|note pane/.test(b.getAttribute('aria-label') ?? '')
    )!;
  async function toggleChat() {
    chatToggle().click();
    await settle();
  }
  /** Let the listeners held back by `h.pendingListeners = []` go live. */
  async function listenersGoLive() {
    for (const goLive of h.pendingListeners!.splice(0)) goLive();
    h.pendingListeners = null;
    await settle();
  }

  /** The runner's end-of-run note write (session_runner.rs) and its event. */
  async function backendWritesNote(next: Row) {
    put(next);
    emit('notes-changed', { branchId: 'branch-1', projectId: null });
    await settle();
  }

  it('1. note → show chat → the agent rewrites the note → hide chat: keeps the rewritten note', async () => {
    put(V1);
    await openFromHost(SNAPSHOT_V1);
    await toggleChat(); // "Show chat"
    await backendWritesNote(V2); // the follow-up turn ends
    expect(noteText()).toContain('NEW body v2'); // the split view shows v2

    await toggleChat(); // "Hide chat"

    expect(noteText()).toContain('NEW body v2');
  });

  /**
   * Chat first on a note its session has not written yet. The store already
   * holds the session's placeholder row, and the dialog takes it up when it
   * opens: with the listener not yet live, nothing else could fetch it.
   */
  async function openPendingNote() {
    put(PLACEHOLDER);
    h.pendingListeners = [];
    const host = await openFromHost(SNAPSHOT_PENDING);

    expect(calls('get_branch_note_by_session')).toEqual([{ sessionId: 'session-1' }]);
    await listenersGoLive();
    // The placeholder is not shown as a note: still the empty state, without
    // the prompt it is titled with, and nothing to move or copy.
    expect(noteText()).toBe('This note has no content.');
    expect(root.textContent).not.toContain(PLACEHOLDER.title);
    expect(transferMenu()).toBeNull();
    // Nor has the user seen it.
    expect(calls('set_note_read')).toEqual([]);
    return host;
  }

  it('2. chat first on a note not written yet → first save → hide chat: keeps the saved note', async () => {
    await openPendingNote();

    await backendWritesNote(FIRST_SAVE);
    // The note pane opens beside the chat, which shows the note.
    expect(noteText()).toContain('First body');
    expect(calls('set_note_read')).toEqual([{ noteId: 'note-1', read: true }]);
    expect(transferMenu()?.dataset.noteId).toBe('note-1');

    await toggleChat();

    expect(noteText()).toContain('First body');
  });

  it('2b. test 2 on a phone / narrow window ("View note")', async () => {
    h.canSplit = false;
    await openPendingNote();

    await backendWritesNote(FIRST_SAVE);
    // Still behind the chat: landed, but not shown yet.
    expect(calls('set_note_read')).toEqual([]);

    await toggleChat();

    expect(noteText()).toContain('First body');
    expect(calls('set_note_read')).toEqual([{ noteId: 'note-1', read: true }]);
  });

  it('2c. a pending note whose session ends without writing it offers the transfer menu', async () => {
    const host = await openPendingNote();

    // The row is still the placeholder, now a failed note rather than one
    // on its way. Its id is the one the open-time fetch brought.
    host.openNote = { ...host.openNote, sessionStatus: 'error' };
    await settle();

    expect(transferMenu()?.dataset.noteId).toBe('note-1');
  });

  it('2d. a note being rewritten keeps the transfer menu, with its session marked active', async () => {
    put(V1);
    await openFromHost({ ...SNAPSHOT_V1, sessionStatus: 'running' });

    expect(transferMenu()?.dataset.noteId).toBe('note-1');
    expect(transferMenu()?.dataset.sessionActive).toBe('');
  });

  it('1b. test 1 on a phone / narrow window ("View chat" → rewrite → "View note")', async () => {
    h.canSplit = false;
    put(V1);
    await openFromHost(SNAPSHOT_V1);
    await toggleChat();
    await backendWritesNote(V2);

    await toggleChat();

    expect(noteText()).toContain('NEW body v2');
  });

  it('3. opened from a row the timeline has not reloaded yet: stays current through a toggle', async () => {
    put(V2); // the store is ahead of the clicked row
    await openFromHost(SNAPSHOT_V1);
    expect(noteText()).toContain('NEW body v2');

    await toggleChat();

    expect(noteText()).toContain('NEW body v2');
  });

  it('4. same stale row, note already read: opening fetches the current note', async () => {
    put({ ...V2, readAt: 2_500 });
    h.pendingListeners = []; // only the open-time fetch, not the listener's
    await openFromHost(SNAPSHOT_V1);

    expect(noteText()).toContain('NEW body v2');
  });

  it('5. a toggle while a refresh is in flight does not drop its result', async () => {
    put(V1);
    await openFromHost(SNAPSHOT_V1);
    const release = hold();
    put(V2);
    emit('notes-changed', { branchId: 'branch-1', projectId: null }); // fetch of v2 now in flight
    await settle();

    await toggleChat(); // host reassigns its snapshot
    release();
    await settle();

    expect(noteText()).toContain('NEW body v2');
  });

  it('6. the chat auto-opening after a refresh keeps the refreshed note', async () => {
    put(V1);
    await openFromHost(SNAPSHOT_V1);
    // Needs a reply newer than the refreshed row; see the note on how rare that is.
    assistantMessagesAfterNote = 1;
    await backendWritesNote(V2);

    expect(noteText()).toContain('NEW body v2');
  });

  it('7. a write that lands before the listener is live is fetched once it is', async () => {
    put(V1);
    h.pendingListeners = [];
    await openFromHost(SNAPSHOT_V1);
    // The open-time fetch has answered with v1; this write's event reaches nobody.
    await backendWritesNote(V2);
    expect(noteText()).toContain('OLD body v1');

    await listenersGoLive();

    expect(noteText()).toContain('NEW body v2');
  });

  it('8. a note without a session is fetched by id on open, and only once', async () => {
    put({ ...V2, readAt: 2_500 });
    h.pendingListeners = []; // only the open-time fetch, not the listener's
    await openFromHost({ ...SNAPSHOT_V1, sessionId: undefined });

    expect(noteText()).toContain('NEW body v2');
    expect(calls('get_note')).toEqual([{ noteId: 'note-1' }]);
  });

  it('9. the host passing an older copy of the same note keeps the fetched one', async () => {
    put(V1);
    const host = await openFromHost(SNAPSHOT_V1);
    await backendWritesNote(V2);

    // A real change of prop values, unlike a toggle — a reference-history
    // entry, say, holding the note as it was when the entry was pushed.
    host.openNote = {
      ...SNAPSHOT_V1,
      title: 'Plan (draft)',
      content: 'OLDER body v0',
      noteUpdatedAt: 500,
    };
    await settle();

    expect(noteText()).toContain('NEW body v2');
  });

  it('10. the host passing a newer copy of the same note shows it', async () => {
    put(V1);
    const host = await openFromHost(SNAPSHOT_V1);
    await backendWritesNote(V2);

    // The props are now the newest copy the modal holds.
    host.openNote = { ...SNAPSHOT_V1, content: 'NEWEST body v3', noteUpdatedAt: 3_000 };
    await settle();

    expect(noteText()).toContain('NEWEST body v3');
  });

  it('11. another note swapped in is shown, then refreshed, not overridden by the last one', async () => {
    put(V1);
    put(OTHER);
    const host = await openFromHost(SNAPSHOT_V1);
    await backendWritesNote(V2);
    const release = hold();
    // Reference navigation swaps notes in place; its entry is older than the store.
    host.openNote = SNAPSHOT_OTHER;
    await settle();
    // Older than the first note's fetched row, but it is not that note.
    expect(noteText()).toContain('OTHER body v1');
    // The swap fetched the note now on screen, not the one it replaced.
    expect(calls('get_branch_note_by_session').at(-1)).toEqual({ sessionId: 'session-2' });

    release();
    await settle();

    expect(noteText()).toContain('OTHER body v2');
  });

  it("12. a fetch still out for the previous note when another is swapped in does not show that note's row", async () => {
    put(V2); // ahead of the clicked row, so the open-time fetch has something to bring
    put(OTHER);
    h.pendingListeners = []; // only the fetches the two notes start
    const releaseFirst = hold();
    const host = await openFromHost(SNAPSHOT_V1);
    expect(noteText()).toContain('OLD body v1'); // its fetch is still out

    const releaseOther = hold();
    host.openNote = SNAPSHOT_OTHER;
    await settle();
    expect(calls('get_branch_note_by_session')).toEqual([
      { sessionId: 'session-1' },
      { sessionId: 'session-2' },
    ]);

    // The first note's fetch answers, late, with the first note's row.
    releaseFirst();
    await settle();
    expect(noteText()).toContain('OTHER body v1');
    expect(noteText()).not.toContain('NEW body v2');

    releaseOther();
    await settle();
    expect(noteText()).toContain('OTHER body v2');
  });

  it('13. a fetch that answers while a later one is still out is shown, not dropped', async () => {
    put(V2);
    h.pendingListeners = [];
    const releaseOpen = hold();
    await openFromHost(SNAPSHOT_V1);
    // A second fetch starts before the first has answered, as the listener's
    // does on every open in web mode.
    hold();
    await listenersGoLive();
    expect(calls('get_branch_note_by_session')).toHaveLength(2);
    expect(noteText()).toContain('OLD body v1');

    releaseOpen();
    await settle();

    expect(noteText()).toContain('NEW body v2');
  });

  it('14. a fetch that answers after a later one has landed does not put its older row back', async () => {
    put(V1);
    h.pendingListeners = [];
    const releaseOpen = hold(); // read v1
    await openFromHost(SNAPSHOT_V1);
    put(V2);
    const releaseLater = hold(); // reads v2
    await listenersGoLive();

    releaseLater();
    await settle();
    expect(noteText()).toContain('NEW body v2');
    releaseOpen();
    await settle();

    expect(noteText()).toContain('NEW body v2');
  });

  it('control: with no host reassignment the refreshed note stays', async () => {
    put(V1);
    await openFromHost(SNAPSHOT_V1);
    await backendWritesNote(V2);
    expect(noteText()).toContain('NEW body v2');
  });
});
