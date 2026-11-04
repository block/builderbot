import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '../../types';

const { sq, createProject, detectDefaultBranch } = vi.hoisted(() => ({
  sq: { loaded: true, available: true },
  createProject: vi.fn(),
  detectDefaultBranch: vi.fn(),
}));

vi.mock('$lib/components/ui/dialog', () =>
  import('../sessions/testStubs/dialogMock').then((m) => m.dialogModuleMock())
);
vi.mock(
  '../projects/ProjectRowContent.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('../settings/sq.svelte', () => ({ sqState: sq }));
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: {
    projects: [],
    reposByProject: new Map(),
    ensureProjectsHydrated: async () => {},
    projectCreated: () => {},
  },
}));
vi.mock('../../api/commands', () => ({ createProject, detectDefaultBranch }));

import NoteTransferDialog from './NoteTransferDialog.svelte';

const created = { id: 'created', name: 'Fresh' } as Project;
const branchRepo = { githubRepo: 'acme/widgets', subpath: 'apps/web' };

let component: ReturnType<typeof mount> | null = null;

function open(repoLabel: typeof branchRepo | null, onConfirm = vi.fn(async () => {})) {
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(NoteTransferDialog, {
    target,
    props: { open: true, mode: 'move', noteTitle: 'Plan', repoLabel, onConfirm },
  });
  flushSync();
  return { target, onConfirm };
}

/** Type a name no project has and pick the "Create project" row it offers. */
function pickCreateRow(target: HTMLElement, name: string) {
  const search = target.querySelector<HTMLInputElement>('input')!;
  search.value = name;
  search.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
  target.querySelector<HTMLButtonElement>('.create-row')!.click();
  flushSync();
}

function locationTile(target: HTMLElement, label: 'Local' | 'Remote') {
  const picker = target.querySelector('[aria-label="Project location"]');
  return [...(picker?.querySelectorAll<HTMLButtonElement>('button') ?? [])].find((b) =>
    b.textContent?.includes(label)
  );
}

function confirm(target: HTMLElement) {
  target.querySelector<HTMLButtonElement>('button[type="submit"]')!.click();
}

describe('NoteTransferDialog new project location', () => {
  beforeEach(() => {
    sq.loaded = true;
    sq.available = true;
    createProject.mockReset().mockResolvedValue(created);
    detectDefaultBranch.mockReset().mockResolvedValue('main');
  });

  afterEach(() => {
    if (component) unmount(component);
    component = null;
    document.body.innerHTML = '';
  });

  it('offers the choice only for a project being created, defaulting to local', async () => {
    const { target, onConfirm } = open(branchRepo);
    expect(locationTile(target, 'Local')).toBeUndefined();

    pickCreateRow(target, 'Fresh');
    expect(locationTile(target, 'Local')?.dataset.state).toBe('on');
    expect(locationTile(target, 'Remote')?.dataset.state).toBe('off');

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'local');
    expect(detectDefaultBranch).not.toHaveBeenCalled();
  });

  it("creates a remote project from the note's repo, so the move adds nothing", async () => {
    const { target, onConfirm } = open(branchRepo);
    pickCreateRow(target, 'Fresh');
    locationTile(target, 'Remote')!.click();
    flushSync();
    expect(locationTile(target, 'Remote')?.dataset.state).toBe('on');

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(detectDefaultBranch).toHaveBeenCalledWith('acme/widgets');
    expect(createProject).toHaveBeenCalledWith(
      'Fresh',
      'remote',
      'acme/widgets',
      'apps/web',
      undefined,
      undefined,
      'main'
    );
  });

  it('keeps the chosen location when its tile is clicked again', () => {
    const { target } = open(branchRepo);
    pickCreateRow(target, 'Fresh');
    locationTile(target, 'Local')!.click();
    flushSync();
    expect(locationTile(target, 'Local')?.dataset.state).toBe('on');
  });

  it('disables remote for a project note, which has no repo to start from', async () => {
    const { target, onConfirm } = open(null);
    pickCreateRow(target, 'Fresh');
    const remote = locationTile(target, 'Remote')!;
    expect(remote.disabled).toBe(true);
    expect(remote.textContent).toContain('Needs a repo; this note has none');

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'local');
  });

  it('offers no choice when remote projects are unavailable', async () => {
    sq.available = false;
    const { target, onConfirm } = open(branchRepo);
    pickCreateRow(target, 'Fresh');
    expect(target.querySelector('[aria-label="Project location"]')).toBeNull();

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'local');
  });
});
