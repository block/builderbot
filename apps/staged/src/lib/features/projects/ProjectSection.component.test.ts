import { flushSync, mount, tick, unmount } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectNote } from '../../types';

const { writer, loadDraftMock } = vi.hoisted(() => ({
  writer: {
    scheduleSave: vi.fn(),
    saveNow: vi.fn(async () => {}),
    clearNow: vi.fn(async () => {}),
    flush: vi.fn(async () => {}),
  },
  loadDraftMock: vi.fn(async (_key: string): Promise<string | null> => null),
}));

// Heavy leaf components are stubbed out: the tests exercise the section's
// project-session draft wiring, not the timeline/branch/note internals. The
// real NewSessionModal is mounted (with its own leaves stubbed) so the open
// paths drive its draft load exactly as in the app.
vi.mock('../branches/BranchCard.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('../timeline/TimelineRow.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock(
  '../timeline/TimelineContextMenu.svelte',
  () => import('../sessions/testStubs/TimelineContextMenuStub.svelte')
);
vi.mock('../notes/NoteModal.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('./SuggestedRepos.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('./ProjectStatusPicker.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
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
  listenToEvent: () => () => {},
}));
vi.mock('../../shared/draftCache', () => ({
  createDraftWriteScheduler: () => writer,
  loadDraft: (key: string) => loadDraftMock(key),
}));
vi.mock('../../api/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/commands')>()),
  listProjectNotes: async () => [completedNote],
}));

import ProjectSection from './ProjectSection.svelte';

const project = {
  id: 'project-1',
  name: 'Project',
  githubRepo: null,
  location: 'local',
  subpath: null,
  createdAt: 1,
  updatedAt: 1,
  statusOverride: null,
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
};

const REF = `#project-note:${completedNote.id}`;
const DRAFT_KEY = `draft:project-session:${project.id}`;

function mountSection() {
  const target = document.createElement('div');
  document.body.appendChild(target);
  return mount(ProjectSection, { target, props: { project, branches: [] } });
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

describe('ProjectSection referring open', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
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
