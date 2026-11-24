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
  noteUpdatedAt?: number;
  chatOpen?: boolean;
}

describe('NoteModal stale note', () => {
  type Row = {
    id: string;
    title: string;
    content: string;
    updatedAt: number;
    readAt: number | null;
  };
  let modal: ReturnType<typeof mount> | null = null;
  let root: HTMLElement;
  /** The note row as the store holds it; the fetches below read it. */
  let stored: Row | null;
  let assistantMessagesAfterNote = 0;
  /** While set, `get_branch_note_by_session` answers only once it resolves. */
  let holdFetch: Promise<void> | null = null;
  /** Every command the modal invoked, in order. */
  let invoked: string[];

  const V1 = {
    id: 'note-1',
    title: 'Plan',
    content: 'OLD body v1',
    updatedAt: 1_000,
    readAt: 1_500,
  };
  const V2 = {
    id: 'note-1',
    title: 'Plan',
    content: 'NEW body v2',
    updatedAt: 2_000,
    readAt: null,
  };
  const SNAPSHOT_V1: OpenNoteState = {
    noteId: 'note-1',
    title: 'Plan',
    content: 'OLD body v1',
    sessionId: 'session-1',
    noteUpdatedAt: 1_000,
  };

  beforeEach(() => {
    document.body.innerHTML = '';
    root = document.createElement('div');
    document.body.appendChild(root);
    h.listeners.clear();
    stored = null;
    assistantMessagesAfterNote = 0;
    holdFetch = null;
    invoked = [];
    h.canSplit = true;
    h.pendingListeners = null;
    h.invoke = async (command) => {
      invoked.push(command);
      // The lookup by id, for a note that has no session.
      if (command === 'get_note') return stored ? { ...stored } : null;
      if (command === 'get_branch_note_by_session') {
        // Read at request time, as the store would, then held like a slow reply.
        const row = stored ? { ...stored } : null;
        if (holdFetch) await holdFetch;
        return row;
      }
      if (command === 'count_assistant_messages_after') return assistantMessagesAfterNote;
      if (command === 'get_media_dimensions') return {};
      if (command === 'set_note_read') {
        // store/notes.rs:323-340: marks the row read and publishes `Notes`
        // only when that changed something.
        const changed = stored!.readAt === null;
        stored = { ...stored!, readAt: 9_999 };
        if (changed)
          queueMicrotask(() => emit('notes-changed', { branchId: 'branch-1', projectId: null }));
        return { ...stored, branchId: 'branch-1', completedAt: 1, parentProjectNoteId: null };
      }
      return null;
    };
  });

  afterEach(() => {
    if (modal) unmount(modal);
    modal = null;
  });

  /** Mount the modal the way BranchCard.svelte:2428-2449 does. */
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
        // BranchCard.svelte:2442-2444, verbatim.
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
  const chatToggle = () =>
    [...root.querySelectorAll<HTMLButtonElement>('button')].find((b) =>
      /chat pane|note pane/.test(b.getAttribute('aria-label') ?? '')
    )!;
  async function toggleChat() {
    chatToggle().click();
    await settle();
  }

  /** The runner's end-of-run note write (session_runner.rs:9228) and its event. */
  async function backendWritesNote(next: Row) {
    stored = next;
    emit('notes-changed', { branchId: 'branch-1', projectId: null });
    await settle();
  }

  it('1. note → show chat → the agent rewrites the note → hide chat: keeps the rewritten note', async () => {
    stored = V1;
    await openFromHost(SNAPSHOT_V1);
    await toggleChat(); // "Show chat"
    await backendWritesNote(V2); // the follow-up turn ends
    expect(noteText()).toContain('NEW body v2'); // the split view shows v2

    await toggleChat(); // "Hide chat"

    expect(noteText()).toContain('NEW body v2');
  });

  it('2. chat first on a note not written yet → first save → hide chat: keeps the saved note', async () => {
    // BranchCard.openNoteChatForSession's pending-note branch (BranchCard.svelte:1117-1122).
    await openFromHost({
      title: 'Generating note',
      content: '',
      sessionId: 'session-1',
      chatOpen: true,
    });
    expect(noteText()).toBe('This note has no content.');
    await backendWritesNote({ ...V2, content: 'First body' });
    expect(noteText()).toContain('First body');

    await toggleChat();

    expect(noteText()).toContain('First body');
  });

  it('2b. test 2 on a phone / narrow window ("View note")', async () => {
    h.canSplit = false;
    await openFromHost({
      title: 'Generating note',
      content: '',
      sessionId: 'session-1',
      chatOpen: true,
    });
    await backendWritesNote({ ...V2, content: 'First body' });

    await toggleChat();

    expect(noteText()).toContain('First body');
  });

  it('1b. test 1 on a phone / narrow window ("View chat" → rewrite → "View note")', async () => {
    h.canSplit = false;
    stored = V1;
    await openFromHost(SNAPSHOT_V1);
    await toggleChat();
    await backendWritesNote(V2);

    await toggleChat();

    expect(noteText()).toContain('NEW body v2');
  });

  it('3. opened from a row the timeline has not reloaded yet: stays current through a toggle', async () => {
    stored = V2; // the store is ahead of the clicked row
    await openFromHost(SNAPSHOT_V1);
    expect(noteText()).toContain('NEW body v2');

    await toggleChat();

    expect(noteText()).toContain('NEW body v2');
  });

  it('4. same stale row, note already read: opening fetches the current note', async () => {
    stored = { ...V2, readAt: 2_500 };
    await openFromHost(SNAPSHOT_V1);

    expect(noteText()).toContain('NEW body v2');
  });

  it('5. a toggle while a refresh is in flight does not drop its result', async () => {
    stored = V1;
    await openFromHost(SNAPSHOT_V1);
    let release!: () => void;
    holdFetch = new Promise<void>((resolve) => (release = resolve));
    stored = V2;
    emit('notes-changed', { branchId: 'branch-1', projectId: null }); // fetch of v2 now in flight
    await settle();

    await toggleChat(); // host reassigns its snapshot
    release();
    await settle();

    expect(noteText()).toContain('NEW body v2');
  });

  it('6. the chat auto-opening after a refresh keeps the refreshed note', async () => {
    stored = V1;
    await openFromHost(SNAPSHOT_V1);
    // Needs a reply newer than the refreshed row; see the note on how rare that is.
    assistantMessagesAfterNote = 1;
    await backendWritesNote(V2);

    expect(noteText()).toContain('NEW body v2');
  });

  it('7. a write that lands before the listener is live is fetched once it is', async () => {
    stored = V1;
    h.pendingListeners = [];
    await openFromHost(SNAPSHOT_V1);
    // The open-time fetch has answered with v1; this write's event reaches nobody.
    await backendWritesNote(V2);
    expect(noteText()).toContain('OLD body v1');

    for (const goLive of h.pendingListeners.splice(0)) goLive();
    await settle();

    expect(noteText()).toContain('NEW body v2');
  });

  it('8. a note without a session is fetched by id on open, and only once', async () => {
    stored = { ...V2, readAt: 2_500 };
    h.pendingListeners = []; // only the open-time fetch, not the listener's
    await openFromHost({ ...SNAPSHOT_V1, sessionId: undefined });

    expect(noteText()).toContain('NEW body v2');
    expect(invoked.filter((command) => command === 'get_note')).toHaveLength(1);
  });

  it('9. the host passing an older copy of the same note keeps the fetched one', async () => {
    stored = V1;
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
    stored = V1;
    const host = await openFromHost(SNAPSHOT_V1);
    await backendWritesNote(V2);

    // The props are now the newest copy the modal holds.
    host.openNote = { ...SNAPSHOT_V1, content: 'NEWEST body v3', noteUpdatedAt: 3_000 };
    await settle();

    expect(noteText()).toContain('NEWEST body v3');
  });

  it('11. another note swapped in is shown, then refreshed, not overridden by the last one', async () => {
    stored = V1;
    const host = await openFromHost(SNAPSHOT_V1);
    await backendWritesNote(V2);
    let release!: () => void;
    holdFetch = new Promise<void>((resolve) => (release = resolve));
    // Reference navigation swaps notes in place; its entry is older than the store.
    stored = {
      id: 'note-2',
      title: 'Other',
      content: 'OTHER body v2',
      updatedAt: 900,
      readAt: 950,
    };
    host.openNote = {
      noteId: 'note-2',
      title: 'Other',
      content: 'OTHER body v1',
      sessionId: 'session-2',
      noteUpdatedAt: 800,
    };
    await settle();
    // Older than the first note's fetched row, but it is not that note.
    expect(noteText()).toContain('OTHER body v1');

    release();
    await settle();

    expect(noteText()).toContain('OTHER body v2');
  });

  it('control: with no host reassignment the refreshed note stays', async () => {
    stored = V1;
    await openFromHost(SNAPSHOT_V1);
    await backendWritesNote(V2);
    expect(noteText()).toContain('NEW body v2');
  });
});
