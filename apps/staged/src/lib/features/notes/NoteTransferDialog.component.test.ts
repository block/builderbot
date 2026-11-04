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

function open(
  repoLabel: typeof branchRepo | null,
  {
    onConfirm = vi.fn(async (_projectId: string) => {}),
    onPrepare,
  }: {
    onConfirm?: (projectId: string) => Promise<void>;
    onPrepare?: () => Promise<void>;
  } = {}
) {
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(NoteTransferDialog, {
    target,
    props: { open: true, mode: 'move', noteTitle: 'Plan', repoLabel, onPrepare, onConfirm },
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

function errorText(target: HTMLElement) {
  return target.querySelector('[role="alert"]')?.textContent?.trim();
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
    expect(detectDefaultBranch).toHaveBeenCalledOnce();
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

  it('fetches the default branch once Remote is picked and awaits that fetch on confirm', async () => {
    let resolveBranch!: (branch: string) => void;
    detectDefaultBranch.mockReturnValue(new Promise((resolve) => (resolveBranch = resolve)));
    const { target, onConfirm } = open(branchRepo);
    pickCreateRow(target, 'Fresh');
    expect(detectDefaultBranch).not.toHaveBeenCalled();

    locationTile(target, 'Remote')!.click();
    flushSync();
    expect(detectDefaultBranch).toHaveBeenCalledWith('acme/widgets');

    // Switching away and back again reuses the fetch already made.
    locationTile(target, 'Local')!.click();
    flushSync();
    locationTile(target, 'Remote')!.click();
    flushSync();

    confirm(target);
    await Promise.resolve();
    expect(createProject).not.toHaveBeenCalled();

    resolveBranch('trunk');
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(detectDefaultBranch).toHaveBeenCalledOnce();
    expect(createProject).toHaveBeenCalledWith(
      'Fresh',
      'remote',
      'acme/widgets',
      'apps/web',
      undefined,
      undefined,
      'trunk'
    );
  });

  it('leaves the default branch to create_project when the prefetch fails', async () => {
    detectDefaultBranch.mockRejectedValue(new Error('GitHub is down'));
    const { target, onConfirm } = open(branchRepo);
    pickCreateRow(target, 'Fresh');
    locationTile(target, 'Remote')!.click();
    flushSync();

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith(
      'Fresh',
      'remote',
      'acme/widgets',
      'apps/web',
      undefined,
      undefined,
      undefined
    );
    expect(errorText(target)).toBeUndefined();
  });
});

describe('NoteTransferDialog pre-transfer checks', () => {
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

  it('runs them before creating the project', async () => {
    const calls: string[] = [];
    createProject.mockImplementation(async () => {
      calls.push('create');
      return created;
    });
    const { target, onConfirm } = open(branchRepo, {
      onPrepare: async () => {
        calls.push('prepare');
      },
      onConfirm: vi.fn(async () => {
        calls.push('transfer');
      }),
    });
    pickCreateRow(target, 'Fresh');
    locationTile(target, 'Remote')!.click();
    flushSync();

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(calls).toEqual(['prepare', 'create', 'transfer']);
  });

  it('creates nothing when they refuse the transfer', async () => {
    const { target, onConfirm } = open(branchRepo, {
      onPrepare: async () => {
        throw new Error('"Plan" is still being generated');
      },
    });
    pickCreateRow(target, 'Fresh');
    locationTile(target, 'Remote')!.click();
    flushSync();

    confirm(target);
    await vi.waitFor(() => expect(errorText(target)).toBe('"Plan" is still being generated'));
    expect(createProject).not.toHaveBeenCalled();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it('says the project was created when the transfer fails after it', async () => {
    const { target, onConfirm } = open(branchRepo, {
      onConfirm: vi.fn(async () => {
        throw new Error('disk full');
      }),
    });
    pickCreateRow(target, 'Fresh');
    locationTile(target, 'Remote')!.click();
    flushSync();

    confirm(target);
    await vi.waitFor(() =>
      expect(errorText(target)).toBe("Created Fresh, but the note wasn't moved: disk full")
    );
    expect(onConfirm).toHaveBeenCalledWith('created');
  });

  it('reports a failure to create the project as it is', async () => {
    createProject.mockRejectedValue(new Error('name taken'));
    const { target, onConfirm } = open(null);
    pickCreateRow(target, 'Fresh');

    confirm(target);
    await vi.waitFor(() => expect(errorText(target)).toBe('name taken'));
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
