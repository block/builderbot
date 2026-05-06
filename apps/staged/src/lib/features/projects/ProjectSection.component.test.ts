import { flushSync, mount, tick, unmount, type ComponentProps } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectNote } from '../../types';
import type { ResumeSessionOutcome } from '../../commands';

const { writer, loadDraftMock, commandMocks, eventHandlers, toastError, toastDismiss } = vi.hoisted(
  () => ({
    writer: {
      scheduleSave: vi.fn(),
      saveNow: vi.fn(async () => {}),
      clearNow: vi.fn(async () => {}),
      flush: vi.fn(async () => {}),
    },
    loadDraftMock: vi.fn(async (_key: string): Promise<string | null> => null),
    commandMocks: {
      listProjectNotes: vi.fn(async (_projectId: string): Promise<ProjectNote[]> => []),
      getProjectNoteBySession: vi.fn(
        async (_sessionId: string): Promise<ProjectNote | null> => null
      ),
      resumeSession: vi.fn(
        async (
          _sessionId: string,
          _prompt: string,
          _imageIds?: string[],
          _branchId?: string | null
        ): Promise<ResumeSessionOutcome> => ({ outcome: 'started' })
      ),
      deleteProjectNote: vi.fn(async (_noteId: string): Promise<void> => {}),
      cancelSession: vi.fn(async (_sessionId: string): Promise<void> => {}),
      listChildSessions: vi.fn(async (_sessionId: string) => []),
    },
    /** The section's transport listeners, keyed by event name. */
    eventHandlers: new Map<string, (payload: unknown) => unknown>(),
    toastError: vi.fn(),
    toastDismiss: vi.fn(),
  })
);

// Heavy leaf components are stubbed out: the tests exercise the section's
// project-session draft wiring and the state it hands its note rows, not the
// timeline/branch/note internals. The real NewSessionModal is mounted (with
// its own leaves stubbed) so the open paths drive its draft load exactly as in
// the app.
vi.mock('../branches/BranchCard.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock(
  '../timeline/TimelineRow.svelte',
  () => import('../sessions/testStubs/TimelineRowStub.svelte')
);
vi.mock(
  '../../shared/ConfirmDeleteDialog.svelte',
  () => import('../sessions/testStubs/ConfirmDeleteDialogStub.svelte')
);
vi.mock(
  '../timeline/TimelineContextMenu.svelte',
  () => import('../sessions/testStubs/TimelineContextMenuStub.svelte')
);
vi.mock('../notes/NoteModal.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('./SuggestedRepos.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('./ProjectTagPills.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('$lib/components/ui/dialog', () =>
  import('../sessions/testStubs/dialogMock').then((m) => m.dialogModuleMock())
);
vi.mock(
  '../sessions/HashtagInput.svelte',
  () => import('../sessions/testStubs/PromptInputStub.svelte')
);
vi.mock(
  '../sessions/ImageAttachment.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('../agents/AcpConfigPicker.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('../branches/dragDrop', () => ({ subscribeDragDrop: () => () => {} }));
vi.mock('../agents/agent.svelte', () => ({ agentState: { providers: [] } }));
vi.mock('../settings/preferences.svelte', () => ({ getPreferredAgent: () => undefined }));
vi.mock('../timeline/liveSessionHints', () => ({
  createLiveSessionHints: () => ({
    syncRunningSessionIds: () => {},
    setPaused: () => {},
    destroy: () => {},
  }),
}));
vi.mock('../sessions/hashtagItems', () => ({
  buildProjectHashtagItems: async () => [],
  projectNotesToHashtagItems: () => [],
}));
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  listenToEvent: (event: string, handler: (payload: unknown) => unknown) => {
    eventHandlers.set(event, handler);
    return () => eventHandlers.delete(event);
  },
}));
vi.mock('svelte-sonner', () => ({ toast: { error: toastError, dismiss: toastDismiss } }));
vi.mock('../../shared/draftCache', () => ({
  createDraftWriteScheduler: () => writer,
  loadDraft: (key: string) => loadDraftMock(key),
}));
// The source module rather than `api/commands`, which only re-exports it:
// `deleteSessionLinkedItem` cancels the note's session through it directly.
vi.mock('../../commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../commands')>()),
  ...commandMocks,
}));

import ProjectSection from './ProjectSection.svelte';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

const project = {
  id: 'project-1',
  name: 'Project',
  githubRepo: null,
  location: 'local',
  subpath: null,
  createdAt: 1,
  updatedAt: 1,
} as Project;

const completedNote: ProjectNote = {
  id: 'note-1',
  projectId: project.id,
  sessionId: 'session-1',
  title: 'A finished note',
  content: 'Note body',
  createdAt: 1,
  updatedAt: 1,
  completedAt: 1,
  suggestedNextCommitStep: null,
  suggestedNextNoteStep: null,
  sessionStatus: 'completed',
  completionReason: null,
  lastChatMessageAt: null,
  chatReadAt: null,
  readAt: null,
};

const REF = `#project-note:${completedNote.id}`;
const DRAFT_KEY = `draft:project-session:${project.id}`;

function mountSection(extra: Partial<ComponentProps<typeof ProjectSection>> = {}) {
  const target = document.createElement('div');
  document.body.appendChild(target);
  return mount(ProjectSection, { target, props: { project, branches: [], ...extra } });
}

/** Let the section's async note/draft loads and effect flushes run to completion. */
async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

function promptInput() {
  return document.querySelector<HTMLElement>('[data-testid="prompt-input"]');
}

function clickReferring() {
  const btn = document.querySelector<HTMLButtonElement>('[data-testid="new-session-referring"]');
  if (!btn) throw new Error('referring trigger not rendered');
  btn.click();
  flushSync();
}

function clickNewProjectNote() {
  const btn = document.querySelector<HTMLButtonElement>('[aria-label="New project note"]');
  if (!btn) throw new Error('new project note button not rendered');
  btn.click();
  flushSync();
}

function savedPrompts(): string[] {
  return [...writer.saveNow.mock.calls, ...writer.scheduleSave.mock.calls].map(
    ([, value]) => value as string
  );
}

function noteWith(overrides: Partial<ProjectNote>): ProjectNote {
  return { ...completedNote, ...overrides };
}

/** A note whose session ended for `reason` before it wrote anything. */
function failedNote(key: string, reason: string, sessionStatus = 'error'): ProjectNote {
  return noteWith({
    id: `note-${key}`,
    sessionId: `session-${key}`,
    title: '',
    content: '',
    sessionStatus,
    completionReason: reason,
  });
}

function listNotes(...notes: ProjectNote[]) {
  commandMocks.listProjectNotes.mockImplementation(async () => notes);
}

/** Answer the next note-by-session refetch with `note`. */
function answerNoteFetch(note: ProjectNote | null) {
  commandMocks.getProjectNoteBySession.mockImplementationOnce(async () => note);
}

/** Hold the next note-by-session refetch open until `release` is called with its result. */
function holdNoteFetch(): (note: ProjectNote | null) => void {
  let release!: (note: ProjectNote | null) => void;
  commandMocks.getProjectNoteBySession.mockImplementationOnce(
    () => new Promise<ProjectNote | null>((resolve) => (release = resolve))
  );
  return (note) => release(note);
}

function rowFor(sessionId: string): HTMLElement {
  const row = document.querySelector<HTMLElement>(
    `[data-testid="timeline-row"][data-session-id="${sessionId}"]`
  );
  if (!row) throw new Error(`no row for ${sessionId}`);
  return row;
}

function resumeButton(sessionId: string): HTMLButtonElement | null {
  return rowFor(sessionId).querySelector<HTMLButtonElement>('[data-testid="row-resume"]');
}

function clickDelete(sessionId: string, opts: { altKey?: boolean } = {}) {
  const btn = rowFor(sessionId).querySelector<HTMLButtonElement>('[data-testid="row-delete"]');
  if (!btn) throw new Error(`no delete button for ${sessionId}`);
  btn.dispatchEvent(new MouseEvent('click', { bubbles: true, altKey: opts.altKey ?? false }));
  flushSync();
}

function confirmDialog(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[data-testid="confirm-delete"]');
}

async function emitSessionStatus(payload: {
  sessionId: string;
  status: string;
  projectId?: string;
  branchId?: string | null;
}) {
  const handler = eventHandlers.get('session-status-changed');
  if (!handler) throw new Error('session-status-changed listener not registered');
  await handler(payload);
  await settle();
}

beforeEach(() => {
  document.body.innerHTML = '';
  eventHandlers.clear();
  toastError.mockReset();
  toastDismiss.mockReset();
  for (const mock of Object.values(commandMocks)) mock.mockReset();
  listNotes(completedNote);
  commandMocks.getProjectNoteBySession.mockImplementation(async () => null);
  commandMocks.resumeSession.mockImplementation(async () => ({ outcome: 'started' }));
  commandMocks.deleteProjectNote.mockImplementation(async () => {});
  commandMocks.cancelSession.mockImplementation(async () => {});
  commandMocks.listChildSessions.mockImplementation(async () => []);
});

describe('ProjectSection referring open', () => {
  beforeEach(() => {
    writer.scheduleSave.mockClear();
    writer.saveNow.mockClear();
    writer.flush.mockClear();
    loadDraftMock.mockReset();
    loadDraftMock.mockImplementation(async () => null);
  });

  it('is superseded by a plain open racing its draft load instead of saving a ref the modal never shows', async () => {
    // Regression: the referring open awaited its flush+read with no supersede
    // guard. A plain "New project note" click landing in that window opened
    // the modal synchronously without the ref; the referring handler then
    // resumed and saved the ref-appended prompt to the store, which the
    // already-open modal never displayed and whose next keystroke overwrote.
    const section = mountSection();
    await settle();

    // The referring open's store read stays in flight…
    let resolveReferringLoad!: (value: string | null) => void;
    loadDraftMock.mockImplementationOnce(
      () => new Promise<string | null>((resolve) => (resolveReferringLoad = resolve))
    );
    clickReferring();
    await vi.waitFor(() => expect(loadDraftMock).toHaveBeenCalledTimes(1));
    expect(promptInput()).toBeNull();

    // …while the user clicks "New project note". The modal opens at once.
    clickNewProjectNote();
    expect(promptInput()).not.toBeNull();
    await settle();

    // The superseded referring open must be a no-op when its read lands: the
    // open modal can't show the ref, so storing it would only be clobbered.
    resolveReferringLoad('stored draft');
    await settle();
    expect(savedPrompts().some((prompt) => prompt.includes(REF))).toBe(false);

    unmount(section);
  });

  it('still builds the referring prompt from the stored draft when unraced', async () => {
    const section = mountSection();
    await settle();

    loadDraftMock.mockImplementation(async () => 'stored draft');
    clickReferring();
    await settle();

    expect(promptInput()).not.toBeNull();
    expect(writer.saveNow).toHaveBeenCalledWith(DRAFT_KEY, `stored draft\n${REF}`);

    unmount(section);
  });
});

describe('ProjectSection note rows', () => {
  it('offers Resume on a crashed note, resumes it with the plain prompt, and refetches it running', async () => {
    const crashed = failedNote('crashed', 'crashed');
    listNotes(crashed);
    const section = mountSection();
    await settle();

    expect(rowFor('session-crashed').dataset.type).toBe('failed-note');
    const resume = resumeButton('session-crashed');
    expect(resume).not.toBeNull();

    answerNoteFetch({ ...crashed, sessionStatus: 'running', completionReason: null });
    resume!.click();
    await settle();

    expect(commandMocks.resumeSession).toHaveBeenCalledWith(
      'session-crashed',
      'Continue where you left off.',
      undefined,
      null
    );
    // Only this note is refetched; the list isn't reloaded.
    expect(commandMocks.getProjectNoteBySession).toHaveBeenCalledWith('session-crashed');
    expect(commandMocks.listProjectNotes).toHaveBeenCalledTimes(1);
    expect(rowFor('session-crashed').dataset.type).toBe('generating-note');
    expect(resumeButton('session-crashed')).toBeNull();

    unmount(section);
  });

  it('names each failure by its reason and offers Resume only for resumable ones', async () => {
    listNotes(
      failedNote('crashed', 'crashed'),
      failedNote('stopped', 'interrupted', 'cancelled'),
      failedNote('quit', 'app_quit'),
      failedNote('empty', 'turn_complete', 'completed'),
      noteWith({
        id: 'note-held',
        sessionId: 'session-held',
        completionReason: 'hold_stopped',
      })
    );
    const section = mountSection();
    await settle();

    const expected: [string, string, string, boolean][] = [
      ['session-crashed', 'failed-note', 'Session crashed — no note created', true],
      ['session-stopped', 'failed-note', 'Session stopped — no note created', true],
      ['session-quit', 'failed-note', 'Session interrupted — no note created', true],
      ['session-empty', 'failed-note', 'Session finished — no note created', false],
      ['session-held', 'note', completedNote.title, true],
    ];
    for (const [sessionId, type, title, resumable] of expected) {
      const row = rowFor(sessionId);
      expect(row.dataset.type, sessionId).toBe(type);
      expect(row.dataset.title, sessionId).toBe(title);
      expect(!!resumeButton(sessionId), sessionId).toBe(resumable);
    }

    unmount(section);
  });

  it('withholds Resume while a resume is in flight, so a double click resumes once', async () => {
    const crashed = failedNote('crashed', 'crashed');
    listNotes(crashed);
    let finishResume!: () => void;
    commandMocks.resumeSession.mockImplementation(
      () =>
        new Promise<ResumeSessionOutcome>(
          (resolve) => (finishResume = () => resolve({ outcome: 'started' }))
        )
    );
    const releaseFetch = holdNoteFetch();
    const section = mountSection();
    await settle();

    resumeButton('session-crashed')!.click();
    flushSync();
    expect(resumeButton('session-crashed')).toBeNull();

    // The button stays down through the refetch that follows the resume too.
    finishResume();
    await settle();
    expect(commandMocks.getProjectNoteBySession).toHaveBeenCalledTimes(1);
    expect(resumeButton('session-crashed')).toBeNull();

    releaseFetch({ ...crashed, sessionStatus: 'running', completionReason: null });
    await settle();
    expect(commandMocks.resumeSession).toHaveBeenCalledTimes(1);
    expect(rowFor('session-crashed').dataset.type).toBe('generating-note');
    expect(resumeButton('session-crashed')).toBeNull();

    unmount(section);
  });

  it('reports a failed resume with a toast and leaves the row resumable', async () => {
    listNotes(failedNote('crashed', 'crashed'));
    commandMocks.resumeSession.mockRejectedValue(new Error('agent unavailable'));
    const section = mountSection();
    await settle();

    resumeButton('session-crashed')!.click();
    await settle();

    expect(toastError).toHaveBeenCalledWith('Resume failed', {
      description: 'agent unavailable',
    });
    expect(commandMocks.getProjectNoteBySession).not.toHaveBeenCalled();
    expect(commandMocks.listProjectNotes).toHaveBeenCalledTimes(1);
    expect(resumeButton('session-crashed')).not.toBeNull();

    unmount(section);
  });

  it('flips a listed note to running when its session is resumed elsewhere', async () => {
    const crashed = failedNote('crashed', 'crashed');
    listNotes(crashed);
    commandMocks.getProjectNoteBySession.mockImplementation(async (sessionId) =>
      sessionId === crashed.sessionId
        ? { ...crashed, sessionStatus: 'running', completionReason: null }
        : null
    );
    const section = mountSection();
    await settle();

    // A repo session on a branch, and a session no listed note owns, leave
    // the rows alone.
    await emitSessionStatus({
      sessionId: 'session-crashed',
      status: 'running',
      projectId: project.id,
      branchId: 'branch-1',
    });
    await emitSessionStatus({
      sessionId: 'session-other',
      status: 'running',
      projectId: project.id,
    });
    expect(commandMocks.getProjectNoteBySession).not.toHaveBeenCalled();

    await emitSessionStatus({
      sessionId: 'session-crashed',
      status: 'running',
      projectId: project.id,
      branchId: null,
    });
    expect(commandMocks.getProjectNoteBySession).toHaveBeenCalledWith('session-crashed');
    expect(rowFor('session-crashed').dataset.type).toBe('generating-note');
    expect(resumeButton('session-crashed')).toBeNull();

    unmount(section);
  });

  it('drops a held running refetch that lands after the terminal one, even across a later refetch', async () => {
    // Regression: refetch numbers were per session and the newest landed
    // fetch deleted its entry, so the next refetch for that session started
    // at 1 again. A slow older fetch could then match the new number and
    // overwrite fresher data, and the fetch it matched was dropped instead.
    const crashed = failedNote('crashed', 'crashed');
    const running = { ...crashed, sessionStatus: 'running', completionReason: null };
    const written = {
      ...crashed,
      title: 'Written on resume',
      content: 'Body',
      sessionStatus: 'completed',
      completionReason: 'turn_complete',
    };
    listNotes(crashed);
    const section = mountSection();
    await settle();
    const handler = eventHandlers.get('session-status-changed')!;

    // A: the session resumes; its running refetch stalls.
    const releaseA = holdNoteFetch();
    const eventA = handler({
      sessionId: 'session-crashed',
      status: 'running',
      projectId: project.id,
    });
    await settle();
    expect(rowFor('session-crashed').dataset.type).toBe('failed-note');

    // B: the session finishes and its terminal refetch lands first.
    answerNoteFetch(written);
    await emitSessionStatus({
      sessionId: 'session-crashed',
      status: 'completed',
      projectId: project.id,
    });
    expect(rowFor('session-crashed').dataset.type).toBe('note');
    expect(rowFor('session-crashed').dataset.title).toBe('Written on resume');

    // C: it is resumed again, and that refetch stalls too.
    const releaseC = holdNoteFetch();
    const eventC = handler({
      sessionId: 'session-crashed',
      status: 'running',
      projectId: project.id,
    });
    await settle();

    // A lands late: stale, so the row keeps B's note.
    releaseA(running);
    await eventA;
    await settle();
    expect(rowFor('session-crashed').dataset.type).toBe('note');
    expect(rowFor('session-crashed').dataset.title).toBe('Written on resume');

    // C lands: it is the latest, so it applies.
    releaseC({ ...written, sessionStatus: 'running', completionReason: null });
    await eventC;
    await settle();
    expect(rowFor('session-crashed').dataset.type).toBe('generating-note');

    unmount(section);
  });

  it('logs a failed refetch and lets the next one through', async () => {
    const crashed = failedNote('crashed', 'crashed');
    listNotes(crashed);
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    const section = mountSection();
    await settle();

    commandMocks.getProjectNoteBySession.mockRejectedValueOnce(new Error('store closed'));
    // The listener resolves rather than rejecting out of the event handler.
    await emitSessionStatus({
      sessionId: 'session-crashed',
      status: 'running',
      projectId: project.id,
    });
    expect(consoleError).toHaveBeenCalledWith(
      '[ProjectSection] Failed to refresh project note:',
      expect.any(Error)
    );
    expect(rowFor('session-crashed').dataset.type).toBe('failed-note');

    answerNoteFetch({ ...crashed, sessionStatus: 'running', completionReason: null });
    await emitSessionStatus({
      sessionId: 'session-crashed',
      status: 'running',
      projectId: project.id,
    });
    expect(rowFor('session-crashed').dataset.type).toBe('generating-note');

    consoleError.mockRestore();
    unmount(section);
  });

  it('asks before deleting a note, and shows it deleting once confirmed', async () => {
    let finishDelete!: () => void;
    commandMocks.deleteProjectNote.mockImplementation(
      () => new Promise<void>((resolve) => (finishDelete = resolve))
    );
    const section = mountSection();
    await settle();

    clickDelete('session-1');
    expect(commandMocks.deleteProjectNote).not.toHaveBeenCalled();
    expect(confirmDialog()?.textContent).toContain(
      'Its linked session and the notes it created on branches will also be deleted.'
    );

    document.querySelector<HTMLButtonElement>('[data-testid="confirm-delete-confirm"]')!.click();
    await settle();
    expect(confirmDialog()).toBeNull();
    expect(commandMocks.cancelSession).toHaveBeenCalledWith('session-1');
    expect(commandMocks.deleteProjectNote).toHaveBeenCalledWith(completedNote.id);
    expect(rowFor('session-1').dataset.secondaryMeta).toBe('Deleting...');
    expect(rowFor('session-1').dataset.deleting).toBe('true');

    finishDelete();
    await settle();
    expect(document.querySelector('[data-testid="timeline-row"]')).toBeNull();

    unmount(section);
  });

  it('keeps the note when the confirmation is cancelled', async () => {
    const section = mountSection();
    await settle();

    clickDelete('session-1');
    document.querySelector<HTMLButtonElement>('[data-testid="confirm-delete-cancel"]')!.click();
    await settle();

    expect(confirmDialog()).toBeNull();
    expect(commandMocks.deleteProjectNote).not.toHaveBeenCalled();
    expect(rowFor('session-1')).not.toBeNull();

    unmount(section);
  });

  it('deletes at once on alt-click and reports a failure with a toast', async () => {
    commandMocks.deleteProjectNote.mockRejectedValue(new Error('database is locked'));
    const section = mountSection();
    await settle();

    clickDelete('session-1', { altKey: true });
    expect(confirmDialog()).toBeNull();
    await settle();

    expect(commandMocks.deleteProjectNote).toHaveBeenCalledWith(completedNote.id);
    expect(toastError).toHaveBeenCalledWith('Failed to delete project note', {
      description: 'database is locked',
      duration: Infinity,
    });
    expect(rowFor('session-1').dataset.deleting).toBe('false');

    unmount(section);
  });
});

// ── Inline project rename ──

function renameButton(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>('[aria-label="Rename project"]');
}

function nameInput(): HTMLInputElement | null {
  return document.querySelector<HTMLInputElement>('[aria-label="Project name"]');
}

async function openRename() {
  const btn = renameButton();
  if (!btn) throw new Error('rename button not rendered');
  btn.click();
  flushSync();
  await settle();
  const input = nameInput();
  if (!input) throw new Error('project name input not rendered');
  return input;
}

function typeName(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
}

function pressKey(input: HTMLInputElement, key: string, init: KeyboardEventInit = {}) {
  input.dispatchEvent(
    new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init })
  );
  flushSync();
}

function blur(input: HTMLInputElement) {
  input.dispatchEvent(new FocusEvent('blur'));
}

const RENAME_TOAST_ID = `rename-project-${project.id}`;

describe('ProjectSection inline rename', () => {
  it('disables the pencil when no rename handler is wired', async () => {
    const section = mountSection();
    await settle();

    expect(renameButton()?.disabled).toBe(true);
    expect(document.querySelector('.project-title')?.textContent).toContain('Project');

    unmount(section);
  });

  it('keeps the pencil out of the heading so its accessible name is the project name', async () => {
    const section = mountSection({ onRenameProject: vi.fn() });
    await settle();

    const heading = document.querySelector<HTMLHeadingElement>('h2.project-title');
    expect(heading?.textContent?.trim()).toBe('Project');
    expect(heading?.querySelector('button')).toBeNull();
    const button = renameButton();
    expect(button?.parentElement).toBe(heading?.parentElement);
    expect(heading?.nextElementSibling).toBe(button);

    unmount(section);
  });

  it('edits the current name in place and saves a changed name on Enter', async () => {
    const onRenameProject = vi.fn(async (_id: string, _name: string) => {});
    const section = mountSection({ onRenameProject });
    await settle();

    const input = await openRename();
    expect(input.value).toBe('Project');
    // The heading stays mounted around the editor so ProjectHome's measured
    // title element keeps its identity.
    expect(input.closest('h2.project-title')).not.toBeNull();

    typeName(input, '  Renamed  ');
    pressKey(input, 'Enter');
    await settle();

    expect(onRenameProject).toHaveBeenCalledWith(project.id, 'Renamed');
    expect(nameInput()).toBeNull();
    expect(toastError).not.toHaveBeenCalled();

    unmount(section);
  });

  it('cancels on Escape or an unchanged name without calling the handler', async () => {
    const onRenameProject = vi.fn(async (_id: string, _name: string) => {});
    const section = mountSection({ onRenameProject });
    await settle();

    let input = await openRename();
    typeName(input, 'Something else');
    pressKey(input, 'Escape');
    expect(nameInput()).toBeNull();

    input = await openRename();
    typeName(input, ' Project ');
    pressKey(input, 'Enter');
    await settle();
    expect(nameInput()).toBeNull();

    expect(onRenameProject).not.toHaveBeenCalled();

    unmount(section);
  });

  it('rejects a blank name inline and keeps editing', async () => {
    const onRenameProject = vi.fn(async (_id: string, _name: string) => {});
    const section = mountSection({ onRenameProject });
    await settle();

    const input = await openRename();
    typeName(input, '   ');
    pressKey(input, 'Enter');
    await settle();

    expect(onRenameProject).not.toHaveBeenCalled();
    expect(nameInput()).not.toBeNull();
    expect(document.querySelector('[role="alert"]')?.textContent).toBe('Project name is required');

    unmount(section);
  });

  it('reports a failed rename with a toast and keeps the editor open', async () => {
    const onRenameProject = vi.fn(async (_id: string, _name: string) => {
      throw new Error('database is locked');
    });
    const section = mountSection({ onRenameProject });
    await settle();

    const input = await openRename();
    typeName(input, 'Renamed');
    pressKey(input, 'Enter');
    await settle();

    expect(onRenameProject).toHaveBeenCalledWith(project.id, 'Renamed');
    expect(toastError).toHaveBeenCalledWith('Unable to rename project', {
      id: RENAME_TOAST_ID,
      description: 'database is locked',
      duration: Infinity,
    });
    const stillEditing = nameInput();
    expect(stillEditing).not.toBeNull();
    expect(stillEditing?.disabled).toBe(false);
    expect(document.querySelector('[role="alert"]')?.textContent).toBe('database is locked');

    unmount(section);
  });

  it('cancels on blur after a failed save instead of re-submitting the same name', async () => {
    const onRenameProject = vi.fn(async (_id: string, _name: string) => {
      throw new Error('database is locked');
    });
    const section = mountSection({ onRenameProject });
    await settle();

    const input = await openRename();
    typeName(input, 'Renamed');
    pressKey(input, 'Enter');
    await settle();
    expect(onRenameProject).toHaveBeenCalledTimes(1);
    expect(toastError).toHaveBeenCalledTimes(1);

    // The editor refocused after the failure; the user clicks away.
    blur(input);
    await settle();

    expect(onRenameProject).toHaveBeenCalledTimes(1);
    expect(toastError).toHaveBeenCalledTimes(1);
    // Giving up takes the pinned failure toast down with the editor.
    expect(toastDismiss).toHaveBeenCalledWith(RENAME_TOAST_ID);
    expect(nameInput()).toBeNull();
    expect(document.querySelector('[role="alert"]')).toBeNull();
    expect(document.querySelector('.project-title-text')?.textContent).toBe('Project');

    unmount(section);
  });

  it('retries a failed name on Enter and replaces the toast rather than stacking one', async () => {
    const onRenameProject = vi.fn(async (_id: string, _name: string) => {
      throw new Error('database is locked');
    });
    const section = mountSection({ onRenameProject });
    await settle();

    const input = await openRename();
    typeName(input, 'Renamed');
    pressKey(input, 'Enter');
    await settle();
    pressKey(input, 'Enter');
    await settle();

    expect(onRenameProject).toHaveBeenCalledTimes(2);
    expect(toastError).toHaveBeenCalledTimes(2);
    const ids = new Set(toastError.mock.calls.map(([, opts]) => (opts as { id: string }).id));
    expect(ids).toEqual(new Set([RENAME_TOAST_ID]));
    expect(nameInput()).not.toBeNull();

    unmount(section);
  });

  it('cancels a blank draft on blur and restores the old name', async () => {
    const onRenameProject = vi.fn(async (_id: string, _name: string) => {});
    const section = mountSection({ onRenameProject });
    await settle();

    const input = await openRename();
    typeName(input, '   ');
    blur(input);
    await settle();

    expect(onRenameProject).not.toHaveBeenCalled();
    expect(nameInput()).toBeNull();
    expect(document.querySelector('[role="alert"]')).toBeNull();
    expect(document.querySelector('.project-title-text')?.textContent).toBe('Project');

    unmount(section);
  });

  it('disables the editor while a rename is pending and closes it exactly once', async () => {
    let resolveRename!: () => void;
    const onRenameProject = vi.fn(
      (_id: string, _name: string) =>
        new Promise<void>((resolve) => {
          resolveRename = resolve;
        })
    );
    const section = mountSection({ onRenameProject });
    await settle();

    const input = await openRename();
    typeName(input, 'Renamed');
    pressKey(input, 'Enter');
    await settle();

    expect(onRenameProject).toHaveBeenCalledTimes(1);
    expect(input.disabled).toBe(true);
    expect(document.querySelector('.project-name-editor .spinner')).not.toBeNull();

    // A second Enter, a blur and an Escape are all ignored until it settles.
    pressKey(input, 'Enter');
    blur(input);
    pressKey(input, 'Escape');
    await settle();
    expect(onRenameProject).toHaveBeenCalledTimes(1);
    expect(nameInput()).toBe(input);
    expect(input.disabled).toBe(true);

    resolveRename();
    await settle();

    expect(nameInput()).toBeNull();
    expect(document.querySelector('.project-name-editor')).toBeNull();
    expect(onRenameProject).toHaveBeenCalledTimes(1);
    expect(toastError).not.toHaveBeenCalled();

    unmount(section);
  });

  it('saves a changed draft on blur and closes the editor', async () => {
    const onRenameProject = vi.fn(async (_id: string, _name: string) => {});
    const section = mountSection({ onRenameProject });
    await settle();

    const input = await openRename();
    typeName(input, ' Renamed on blur ');
    blur(input);
    await settle();

    expect(onRenameProject).toHaveBeenCalledWith(project.id, 'Renamed on blur');
    expect(nameInput()).toBeNull();
    expect(document.querySelector('.project-name-editor')).toBeNull();
    expect(toastError).not.toHaveBeenCalled();

    unmount(section);
  });

  it('closes the editor without renaming when a delete starts while it is open', async () => {
    const onRenameProject = vi.fn(async (_id: string, _name: string) => {});
    const props = reactiveProps({ project, branches: [], deleting: false, onRenameProject });
    const target = document.createElement('div');
    document.body.appendChild(target);
    const section = mount(ProjectSection, { target, props });
    await settle();

    const input = await openRename();
    typeName(input, 'Renamed');

    props.deleting = true;
    flushSync();
    // Disabling the input blurs it in a real browser; that blur must not save.
    blur(input);
    await settle();

    expect(onRenameProject).not.toHaveBeenCalled();
    expect(nameInput()).toBeNull();
    expect(document.querySelector('.project-title-text')?.textContent).toBe('Project');
    expect(renameButton()?.disabled).toBe(true);

    unmount(section);
  });

  it('ignores the Enter that confirms an IME composition', async () => {
    const onRenameProject = vi.fn(async (_id: string, _name: string) => {});
    const section = mountSection({ onRenameProject });
    await settle();

    const input = await openRename();
    typeName(input, 'にほ');
    pressKey(input, 'Enter', { isComposing: true });
    await settle();

    expect(onRenameProject).not.toHaveBeenCalled();
    expect(nameInput()).toBe(input);

    // The real Enter after the composition has ended still submits.
    typeName(input, '日本');
    pressKey(input, 'Enter');
    await settle();
    expect(onRenameProject).toHaveBeenCalledWith(project.id, '日本');
    expect(nameInput()).toBeNull();

    unmount(section);
  });

  it('dismisses the pinned failure toast once a retry succeeds', async () => {
    const onRenameProject = vi
      .fn(async (_id: string, _name: string) => {})
      .mockRejectedValueOnce(new Error('database is locked'));
    const section = mountSection({ onRenameProject });
    await settle();

    const input = await openRename();
    typeName(input, 'Renamed');
    pressKey(input, 'Enter');
    await settle();
    expect(toastError).toHaveBeenCalledTimes(1);
    expect(toastDismiss).not.toHaveBeenCalled();

    pressKey(input, 'Enter');
    await settle();

    expect(onRenameProject).toHaveBeenCalledTimes(2);
    expect(toastDismiss).toHaveBeenCalledWith(RENAME_TOAST_ID);
    expect(nameInput()).toBeNull();

    unmount(section);
  });

  it('dismisses the pinned failure toast when the editor is cancelled with Escape', async () => {
    const onRenameProject = vi.fn(async (_id: string, _name: string) => {
      throw new Error('database is locked');
    });
    const section = mountSection({ onRenameProject });
    await settle();

    const input = await openRename();
    typeName(input, 'Renamed');
    pressKey(input, 'Enter');
    await settle();
    expect(toastError).toHaveBeenCalledTimes(1);

    pressKey(input, 'Escape');
    await settle();

    expect(toastDismiss).toHaveBeenCalledWith(RENAME_TOAST_ID);
    expect(nameInput()).toBeNull();

    unmount(section);
  });
});
