import { flushSync, mount, tick, unmount } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { BranchNote, Session } from '../../types';

const { branchNote } = vi.hoisted(() => ({
  /** What `getBranchNoteBySession` answers for the mounted session. */
  branchNote: { current: null as BranchNote | null, fail: false },
}));

// Heavy leaf components are stubbed out: these tests exercise whether the
// pane offers a composer at all, not the composer's internals.
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
  createDraftWriteScheduler: () => ({
    scheduleSave: vi.fn(),
    saveNow: vi.fn(async () => {}),
    clearNow: vi.fn(async () => {}),
    flush: vi.fn(async () => {}),
  }),
  loadDraft: async () => null,
}));
vi.mock('../../commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../commands')>()),
  getSession: async (id: string) =>
    ({
      id,
      status: 'completed',
      completionReason: 'interrupted',
      provider: null,
      workingDir: null,
    }) as unknown as Session,
  getLatestSessionMessages: async () => ({ data: [] }),
  getSessionMessages: async () => ({ data: [] }),
  getSessionMessagesSince: async () => [],
  getSessionAcpMetadataMessages: async () => [],
  getSessionAcpMetadataMessagesSince: async () => [],
  listQueuedSessionMessages: async () => [],
  getBranchNoteBySession: async () => {
    if (branchNote.fail) throw new Error('lookup failed');
    return branchNote.current;
  },
}));

import SessionChatPane from './SessionChatPane.svelte';
import { reactiveProps } from './testStubs/reactiveProps.svelte';

function note(sessionId: string, parentProjectNoteId: string | null): BranchNote {
  return {
    id: `note-${sessionId}`,
    branchId: 'branch-1',
    sessionId,
    title: 'Note',
    content: 'body',
    createdAt: 1,
    updatedAt: 1,
    completedAt: 1,
    suggestedNextCommitStep: null,
    suggestedNextNoteStep: null,
    parentProjectNoteId,
    subtype: null,
    readAt: null,
  };
}

function mountPane(sessionId: string) {
  const props = reactiveProps({ active: true, sessionId, hashtagItems: [] });
  const target = document.createElement('div');
  document.body.appendChild(target);
  const pane = mount(SessionChatPane, { target, props });
  return { pane, props };
}

/** Let the pane's async session and note loads and effect flushes settle. */
async function settle() {
  for (let i = 0; i < 8; i++) {
    await tick();
  }
  flushSync();
}

const composer = () => document.querySelector('[data-testid="prompt-input"]');
const readOnlyNotice = () => document.querySelector('[data-testid="read-only-chat-notice"]');
const resumeButton = () =>
  Array.from(document.querySelectorAll('button')).find((b) => b.textContent?.trim() === 'Resume');

describe('SessionChatPane for a hidden child note', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    branchNote.current = null;
    branchNote.fail = false;
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  it('replaces the composer and Resume with a read-only notice', async () => {
    branchNote.current = note('child', 'project-note-1');
    const { pane } = mountPane('child');
    flushSync();
    await settle();

    expect(composer()).toBeNull();
    expect(resumeButton()).toBeUndefined();
    expect(readOnlyNotice()?.textContent).toContain('read-only');
    expect(readOnlyNotice()?.textContent).toContain('project note');

    unmount(pane);
  });

  it("keeps the composer for a note that is shown in the branch's notes", async () => {
    branchNote.current = note('standalone', null);
    const { pane } = mountPane('standalone');
    flushSync();
    await settle();

    expect(composer()).not.toBeNull();
    expect(resumeButton()).toBeDefined();
    expect(readOnlyNotice()).toBeNull();

    unmount(pane);
  });

  it('keeps the composer when the note lookup fails or finds no note', async () => {
    branchNote.fail = true;
    const { pane, props } = mountPane('no-note');
    flushSync();
    await settle();
    expect(composer()).not.toBeNull();
    expect(readOnlyNotice()).toBeNull();

    // A commit session has no note; the chat is writable.
    branchNote.fail = false;
    props.sessionId = 'commit';
    flushSync();
    await settle();
    expect(composer()).not.toBeNull();
    expect(readOnlyNotice()).toBeNull();

    unmount(pane);
  });

  it('drops the notice when the pane moves on to a writable session', async () => {
    branchNote.current = note('child', 'project-note-1');
    const { pane, props } = mountPane('child');
    flushSync();
    await settle();
    expect(readOnlyNotice()).not.toBeNull();

    branchNote.current = note('standalone', null);
    props.sessionId = 'standalone';
    flushSync();
    await settle();
    expect(readOnlyNotice()).toBeNull();
    expect(composer()).not.toBeNull();

    unmount(pane);
  });
});
