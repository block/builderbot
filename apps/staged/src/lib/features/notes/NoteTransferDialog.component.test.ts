import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '../../types';

const { sq, store, createProject, detectDefaultBranch } = vi.hoisted(() => ({
  sq: { loaded: true, available: true },
  store: { projects: [] as Project[] },
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
    get projects() {
      return store.projects;
    },
    reposByProject: new Map(),
    ensureProjectsHydrated: async () => {},
    projectCreated: () => {},
  },
}));
vi.mock('../../api/commands', () => ({ createProject, detectDefaultBranch }));

import NoteTransferDialog from './NoteTransferDialog.svelte';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';
import type { NoteRepo } from './noteTransferTarget';

const created = { id: 'created', name: 'Fresh' } as Project;
const branchRepo = { githubRepo: 'acme/widgets', subpath: 'apps/web' };

let component: ReturnType<typeof mount> | null = null;

/** A branch note's lookup that finds `repo`. */
function repoLookup(repo: NoteRepo | null) {
  return vi.fn(async () => repo);
}

/** A lookup whose answers land when the test says, one per call. */
function deferredLookup() {
  const calls: { land: (repo: NoteRepo | null) => void; fail: (e: unknown) => void }[] = [];
  const lookup = vi.fn(
    () => new Promise<NoteRepo | null>((land, fail) => void calls.push({ land, fail }))
  );
  return { lookup, calls };
}

/** Let pending promise callbacks run, then render what they changed. */
async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
  flushSync();
}

/**
 * Open the picker for a branch note whose stored repo is `repo` (`null`: it
 * has none), or for a project note when `repo` is `'project'` — which, as
 * NoteTransferMenu does, gets no lookup at all.
 */
async function open(
  repo: NoteRepo | null | 'project' | (() => Promise<NoteRepo | null>),
  {
    onConfirm = vi.fn(async (_projectId: string) => {}),
    onPrepare,
  }: {
    onConfirm?: (projectId: string) => Promise<void>;
    onPrepare?: () => Promise<void>;
  } = {}
) {
  const loadNoteRepo =
    repo === 'project' ? undefined : typeof repo === 'function' ? repo : repoLookup(repo);
  const target = document.createElement('div');
  document.body.appendChild(target);
  const props = reactiveProps({
    open: true,
    mode: 'move' as const,
    noteTitle: 'Plan',
    loadNoteRepo,
    onPrepare,
    onConfirm,
  });
  component = mount(NoteTransferDialog, { target, props });
  flushSync();
  await settle();
  return { target, props, onConfirm };
}

function type(target: HTMLElement, query: string) {
  const search = target.querySelector<HTMLInputElement>('input')!;
  search.value = query;
  search.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
}

/** Type a name no project has and pick the "Create project" row it offers. */
function pickCreateRow(target: HTMLElement, name: string) {
  type(target, name);
  target.querySelector<HTMLButtonElement>('.create-row')!.click();
  flushSync();
}

function chooseRemote(target: HTMLElement) {
  locationTile(target, 'Remote')!.click();
  flushSync();
}

function existingRow(target: HTMLElement) {
  return target.querySelector<HTMLButtonElement>('.project-row:not(.create-row)')!;
}

function confirmButton(target: HTMLElement) {
  return target.querySelector<HTMLButtonElement>('button[type="submit"]')!;
}

function locationTile(target: HTMLElement, label: 'Local' | 'Remote') {
  const picker = target.querySelector('[aria-label="Project location"]');
  return [...(picker?.querySelectorAll<HTMLButtonElement>('button') ?? [])].find((b) =>
    b.textContent?.includes(label)
  );
}

function confirm(target: HTMLElement) {
  confirmButton(target).click();
}

function errorText(target: HTMLElement) {
  return target.querySelector('[role="alert"]')?.textContent?.trim();
}

describe('NoteTransferDialog new project location', () => {
  beforeEach(() => {
    sq.loaded = true;
    sq.available = true;
    store.projects = [];
    createProject.mockReset().mockResolvedValue(created);
    detectDefaultBranch.mockReset().mockResolvedValue('main');
  });

  afterEach(() => {
    if (component) unmount(component);
    component = null;
    document.body.innerHTML = '';
  });

  it('offers the choice only for a project being created, defaulting to local', async () => {
    const { target, onConfirm } = await open(branchRepo);
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
    const { target, onConfirm } = await open(branchRepo);
    pickCreateRow(target, 'Fresh');
    chooseRemote(target);
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

  it('keeps the chosen location when its tile is clicked again', async () => {
    const { target } = await open(branchRepo);
    pickCreateRow(target, 'Fresh');
    locationTile(target, 'Local')!.click();
    flushSync();
    expect(locationTile(target, 'Local')?.dataset.state).toBe('on');
  });

  it('disables remote for a project note, which has no repo to start from', async () => {
    const { target, onConfirm } = await open('project');
    pickCreateRow(target, 'Fresh');
    const remote = locationTile(target, 'Remote')!;
    expect(remote.disabled).toBe(true);
    expect(remote.textContent).toContain('Needs a repo; this note has none');

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'local');
  });

  it('offers the choice for a typed name that confirming would create, unpicked', async () => {
    const { target, onConfirm } = await open(branchRepo);
    type(target, 'Fresh');
    // The create row is all that's left, so it is what Move (or Enter) acts on.
    expect(target.querySelector('.create-row')?.getAttribute('aria-checked')).toBe('true');
    expect(confirmButton(target).disabled).toBe(false);
    expect(locationTile(target, 'Local')?.dataset.state).toBe('on');

    chooseRemote(target);
    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
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

  it('shows the choice again when a picked project is typed away', async () => {
    store.projects = [{ id: 'widgets', name: 'Widgets' } as Project];
    const { target, onConfirm } = await open(branchRepo);
    pickCreateRow(target, 'Wid');
    chooseRemote(target);
    existingRow(target).click();
    flushSync();
    expect(target.querySelector('[aria-label="Project location"]')).toBeNull();

    // Widgets no longer matches, so confirming would create "Widx" — remote,
    // as last chosen, and on screen to say so.
    type(target, 'Widx');
    expect(locationTile(target, 'Remote')?.dataset.state).toBe('on');
    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith(
      'Widx',
      'remote',
      'acme/widgets',
      'apps/web',
      undefined,
      undefined,
      'main'
    );
  });

  it('offers no choice while confirming would not create a project', async () => {
    store.projects = [{ id: 'widgets', name: 'Widgets' } as Project];
    const { target } = await open(branchRepo);
    // Widgets and the create row are both on offer: nothing to act on yet.
    type(target, 'Wid');
    expect(confirmButton(target).disabled).toBe(true);
    expect(target.querySelector('[aria-label="Project location"]')).toBeNull();

    // Typed in full, Widgets is the only row left: Move acts on it, and it
    // already exists.
    type(target, 'Widgets');
    expect(existingRow(target).getAttribute('aria-checked')).toBe('true');
    expect(confirmButton(target).disabled).toBe(false);
    expect(target.querySelector('[aria-label="Project location"]')).toBeNull();
  });

  it('acts on nothing until something is picked or typed, even with one project on offer', async () => {
    store.projects = [{ id: 'widgets', name: 'Widgets' } as Project];
    const { target, onConfirm } = await open(branchRepo);
    expect(existingRow(target).getAttribute('aria-checked')).toBe('false');
    expect(confirmButton(target).disabled).toBe(true);

    // Enter straight after opening submits the form with nothing to act on.
    target.querySelector('form')!.requestSubmit();
    await settle();
    expect(onConfirm).not.toHaveBeenCalled();

    // Clearing a query leaves nothing assumed either.
    type(target, 'Widgets');
    type(target, '');
    expect(existingRow(target).getAttribute('aria-checked')).toBe('false');
    expect(confirmButton(target).disabled).toBe(true);
  });

  it('offers no choice when remote projects are unavailable', async () => {
    sq.available = false;
    const { target, onConfirm } = await open(branchRepo);
    pickCreateRow(target, 'Fresh');
    expect(target.querySelector('[aria-label="Project location"]')).toBeNull();

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'local');
  });

  it('fetches the default branch once Remote is picked and awaits that fetch on confirm', async () => {
    let resolveBranch!: (branch: string) => void;
    detectDefaultBranch.mockReturnValue(new Promise((resolve) => (resolveBranch = resolve)));
    const { target, onConfirm } = await open(branchRepo);
    pickCreateRow(target, 'Fresh');
    expect(detectDefaultBranch).not.toHaveBeenCalled();

    chooseRemote(target);
    expect(detectDefaultBranch).toHaveBeenCalledWith('acme/widgets');

    // Switching away and back again reuses the fetch already made.
    locationTile(target, 'Local')!.click();
    flushSync();
    chooseRemote(target);

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
    const { target, onConfirm } = await open(branchRepo);
    pickCreateRow(target, 'Fresh');
    chooseRemote(target);

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

describe("NoteTransferDialog note's repo", () => {
  beforeEach(() => {
    sq.loaded = true;
    sq.available = true;
    store.projects = [];
    createProject.mockReset().mockResolvedValue(created);
    detectDefaultBranch.mockReset().mockResolvedValue('main');
  });

  afterEach(() => {
    if (component) unmount(component);
    component = null;
    document.body.innerHTML = '';
  });

  it('holds Remote until the lookup lands, then starts from the repo it found', async () => {
    const { lookup, calls } = deferredLookup();
    const { target, onConfirm } = await open(lookup);
    pickCreateRow(target, 'Fresh');
    const remote = locationTile(target, 'Remote')!;
    expect(remote.disabled).toBe(true);
    expect(remote.textContent).toContain("Checking this note's repo…");

    calls[0].land({ githubRepo: 'acme/gadgets', subpath: null });
    await settle();
    expect(locationTile(target, 'Remote')!.disabled).toBe(false);
    chooseRemote(target);
    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(detectDefaultBranch).toHaveBeenCalledWith('acme/gadgets');
    expect(createProject).toHaveBeenCalledWith(
      'Fresh',
      'remote',
      'acme/gadgets',
      undefined,
      undefined,
      undefined,
      'main'
    );
  });

  it('keeps a branch note bound to no repo local', async () => {
    const { target } = await open(null);
    pickCreateRow(target, 'Fresh');
    const remote = locationTile(target, 'Remote')!;
    expect(remote.disabled).toBe(true);
    expect(remote.textContent).toContain('Needs a repo; this note has none');
  });

  it('says the lookup failed rather than that the note has no repo', async () => {
    const { lookup, calls } = deferredLookup();
    const { target, onConfirm } = await open(lookup);
    calls[0].fail(new Error('Note not found: note-1'));
    await settle();
    pickCreateRow(target, 'Fresh');
    const remote = locationTile(target, 'Remote')!;
    expect(remote.disabled).toBe(true);
    expect(remote.textContent).toContain("Couldn't look up this note's repo");

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'local');
  });

  it('looks again on each open, dropping a late answer from the last one', async () => {
    const { lookup, calls } = deferredLookup();
    const { target, props } = await open(lookup);
    props.open = false;
    flushSync();
    props.open = true;
    flushSync();
    expect(lookup).toHaveBeenCalledTimes(2);

    calls[1].land(branchRepo);
    await settle();
    calls[0].land(null);
    await settle();
    pickCreateRow(target, 'Fresh');
    expect(locationTile(target, 'Remote')!.disabled).toBe(false);
  });
});

describe('NoteTransferDialog pre-transfer checks', () => {
  beforeEach(() => {
    sq.loaded = true;
    sq.available = true;
    store.projects = [];
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
    const { target, onConfirm } = await open(branchRepo, {
      onPrepare: async () => {
        calls.push('prepare');
      },
      onConfirm: vi.fn(async () => {
        calls.push('transfer');
      }),
    });
    pickCreateRow(target, 'Fresh');
    chooseRemote(target);

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(calls).toEqual(['prepare', 'create', 'transfer']);
  });

  it('creates nothing when they refuse the transfer', async () => {
    const { target, onConfirm } = await open(branchRepo, {
      onPrepare: async () => {
        throw new Error('"Plan" is still being generated');
      },
    });
    pickCreateRow(target, 'Fresh');
    chooseRemote(target);

    confirm(target);
    await vi.waitFor(() => expect(errorText(target)).toBe('"Plan" is still being generated'));
    expect(createProject).not.toHaveBeenCalled();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it('says the project was created when the transfer fails after it', async () => {
    const { target, onConfirm } = await open(branchRepo, {
      onConfirm: vi.fn(async () => {
        throw new Error('disk full');
      }),
    });
    pickCreateRow(target, 'Fresh');
    chooseRemote(target);

    confirm(target);
    await vi.waitFor(() =>
      expect(errorText(target)).toBe("Created Fresh, but the note wasn't moved: disk full")
    );
    expect(onConfirm).toHaveBeenCalledWith('created');
  });

  it('reports a failure to create the project as it is', async () => {
    createProject.mockRejectedValue(new Error('name taken'));
    const { target, onConfirm } = await open('project');
    pickCreateRow(target, 'Fresh');

    confirm(target);
    await vi.waitFor(() => expect(errorText(target)).toBe('name taken'));
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
