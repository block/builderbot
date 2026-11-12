import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectLocation } from '../../types';

const { sq, store, prefs, setNewProjectLocation, createProject, detectDefaultBranch } = vi.hoisted(
  () => {
    const prefs = { newProjectLocation: 'local' as ProjectLocation };
    return {
      sq: { loaded: true, available: true },
      store: { projects: [] as Project[], deleting: new Set<string>() },
      prefs,
      setNewProjectLocation: vi.fn((location: ProjectLocation) => {
        prefs.newProjectLocation = location;
      }),
      createProject: vi.fn(),
      detectDefaultBranch: vi.fn(),
    };
  }
);

vi.mock('$lib/components/ui/dialog', () =>
  import('../sessions/testStubs/dialogMock').then((m) => m.dialogModuleMock())
);
vi.mock(
  '../projects/ProjectRowContent.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('../settings/sq.svelte', () => ({ bloxAvailable: () => sq.loaded && sq.available }));
vi.mock('../settings/preferences.svelte', () => ({ preferences: prefs, setNewProjectLocation }));
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: {
    get projects() {
      return store.projects;
    },
    get liveProjects() {
      return store.projects.filter((p) => !store.deleting.has(p.id));
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
    hasChildren = false,
  }: {
    onConfirm?: (projectId: string) => Promise<void>;
    onPrepare?: () => Promise<void>;
    hasChildren?: boolean;
  } = {}
) {
  const loadNoteRepo =
    repo === 'project' ? undefined : typeof repo === 'function' ? repo : repoLookup(repo);
  const target = document.createElement('div');
  document.body.appendChild(target);
  const props = reactiveProps({
    open: true,
    mode: 'move' as const,
    noteKind: repo === 'project' ? ('project' as const) : ('branch' as const),
    noteTitle: 'Plan',
    loadNoteRepo,
    hasChildren,
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

/**
 * Remote is disabled but keeps its own description; the reason is a readable
 * line below the tiles, outside the faded tile, that the tile and group point to.
 */
function expectRemoteBlocked(target: HTMLElement, reason: string) {
  const picker = target.querySelector('[aria-label="Project location"]')!;
  const remote = locationTile(target, 'Remote')!;
  expect(remote.disabled).toBe(true);
  expect(remote.textContent).toContain('Run agents in the cloud');
  expect(remote.textContent).not.toContain(reason);
  const id = remote.getAttribute('aria-describedby');
  expect(id).toBeTruthy();
  expect(picker.getAttribute('aria-describedby')).toBe(id);
  const line = document.getElementById(id!);
  expect(line?.textContent).toBe(reason);
  expect(picker.contains(line)).toBe(false);
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
    prefs.newProjectLocation = 'local';
    setNewProjectLocation.mockClear();
    store.deleting = new Set();
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
    expect(setNewProjectLocation).toHaveBeenCalledExactlyOnceWith('local');
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

  it('starts on the remembered location and remembers the one it creates with', async () => {
    prefs.newProjectLocation = 'remote';
    const { target, props, onConfirm } = await open(branchRepo);
    pickCreateRow(target, 'Fresh');
    expect(locationTile(target, 'Remote')?.dataset.state).toBe('on');
    // Choosing doesn't commit it; creating does.
    locationTile(target, 'Local')!.click();
    flushSync();
    chooseRemote(target);
    expect(setNewProjectLocation).not.toHaveBeenCalled();

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
    expect(setNewProjectLocation).toHaveBeenCalledExactlyOnceWith('remote');
    expect(props.open).toBe(false);

    // Picking Local next time makes Local the one the open after starts on.
    props.open = true;
    flushSync();
    await settle();
    pickCreateRow(target, 'Second');
    expect(locationTile(target, 'Remote')?.dataset.state).toBe('on');
    locationTile(target, 'Local')!.click();
    flushSync();
    confirm(target);
    await vi.waitFor(() => expect(createProject).toHaveBeenLastCalledWith('Second', 'local'));
    expect(setNewProjectLocation).toHaveBeenLastCalledWith('local');
    expect(prefs.newProjectLocation).toBe('local');
  });

  it('shows a remembered Remote as local while blocked, and keeps remembering it', async () => {
    prefs.newProjectLocation = 'remote';
    const { target, onConfirm } = await open(() => Promise.reject(new Error('Note not found')));
    pickCreateRow(target, 'Fresh');
    expectRemoteBlocked(target, "Remote is unavailable: couldn't look up this note's repo");
    expect(locationTile(target, 'Local')?.dataset.state).toBe('on');
    expect(locationTile(target, 'Remote')?.dataset.state).toBe('off');

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'local');
    // Local was the only option, not a choice, so Remote stays remembered.
    expect(setNewProjectLocation).not.toHaveBeenCalled();
  });

  it('creates locally without forgetting a remembered Remote when remote is unavailable', async () => {
    sq.available = false;
    prefs.newProjectLocation = 'remote';
    const { target, onConfirm } = await open(branchRepo);
    pickCreateRow(target, 'Fresh');
    expect(target.querySelector('[aria-label="Project location"]')).toBeNull();

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'local');
    expect(setNewProjectLocation).not.toHaveBeenCalled();
  });

  it('keeps the chosen location when its tile is clicked again', async () => {
    const { target } = await open(branchRepo);
    pickCreateRow(target, 'Fresh');
    locationTile(target, 'Local')!.click();
    flushSync();
    expect(locationTile(target, 'Local')?.dataset.state).toBe('on');
  });

  it('creates a remote project with no repo for a project note, which has none', async () => {
    const { target, onConfirm } = await open('project');
    pickCreateRow(target, 'Fresh');
    expect(locationTile(target, 'Remote')!.disabled).toBe(false);
    chooseRemote(target);

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'remote');
    expect(detectDefaultBranch).not.toHaveBeenCalled();
    expect(setNewProjectLocation).toHaveBeenCalledExactlyOnceWith('remote');
  });

  it('creates locally for a project note whose child notes move with it', async () => {
    prefs.newProjectLocation = 'remote';
    const { target, onConfirm } = await open('project', { hasChildren: true });
    pickCreateRow(target, 'Fresh');
    expectRemoteBlocked(target, 'Child notes can only move to a local project');
    expect(locationTile(target, 'Local')?.dataset.state).toBe('on');

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'local');
    expect(setNewProjectLocation).not.toHaveBeenCalled();
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

  it('offers no "Create project" under the name of a project still being deleted', async () => {
    store.projects = [
      { id: 'widgets', name: 'Widgets' } as Project,
      { id: 'gadgets', name: 'Gadgets' } as Project,
    ];
    store.deleting = new Set(['gadgets']);
    const { target } = await open(branchRepo);
    // Off the destinations as soon as its delete started...
    expect(target.querySelectorAll('.project-row:not(.create-row)')).toHaveLength(1);

    // ...but its row, and so its unique name, lasts until the teardown ends:
    // creating "Gadgets" now would only fail.
    type(target, 'Gadgets');
    expect(target.querySelector('.create-row')).toBeNull();
    expect(target.querySelector('.project-row')).toBeNull();
    expect(confirmButton(target).disabled).toBe(true);

    type(target, 'Gadgets 2');
    expect(target.querySelector('.create-row')).not.toBeNull();
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

  it('fetches no default branch for a move into an existing project', async () => {
    prefs.newProjectLocation = 'remote';
    store.projects = [{ id: 'widgets', name: 'Widgets' } as Project];
    const { target, onConfirm } = await open(branchRepo);
    existingRow(target).click();
    flushSync();

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('widgets'));
    expect(detectDefaultBranch).not.toHaveBeenCalled();
    expect(createProject).not.toHaveBeenCalled();
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
    prefs.newProjectLocation = 'local';
    setNewProjectLocation.mockClear();
    store.deleting = new Set();
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
    expectRemoteBlocked(target, "Checking this note's repo for Remote…");

    calls[0].land({ githubRepo: 'acme/gadgets', subpath: null });
    await settle();
    expect(locationTile(target, 'Remote')!.disabled).toBe(false);
    expect(locationTile(target, 'Remote')!.hasAttribute('aria-describedby')).toBe(false);
    expect(target.textContent).not.toContain("Checking this note's repo for Remote…");
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

  it('switches to a remembered Remote once the lookup lands', async () => {
    prefs.newProjectLocation = 'remote';
    const { lookup, calls } = deferredLookup();
    const { target, onConfirm } = await open(lookup);
    pickCreateRow(target, 'Fresh');
    expect(locationTile(target, 'Local')?.dataset.state).toBe('on');
    expect(detectDefaultBranch).not.toHaveBeenCalled();

    calls[0].land(branchRepo);
    await settle();
    expect(locationTile(target, 'Remote')?.dataset.state).toBe('on');
    expect(detectDefaultBranch).toHaveBeenCalledWith('acme/widgets');
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

  it('creates the Local it showed when the lookup lands while confirming', async () => {
    prefs.newProjectLocation = 'remote';
    const { lookup, calls } = deferredLookup();
    const { target, onConfirm } = await open(lookup, {
      // As NoteTransferMenu's checks do, wait on the lookup the picker is
      // waiting on, which lands for the picker first.
      onPrepare: async () => {
        calls[0].land(branchRepo);
        await lookup.mock.results[0].value;
      },
    });
    pickCreateRow(target, 'Fresh');
    expect(locationTile(target, 'Local')?.dataset.state).toBe('on');

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'local');
    // Remote wasn't on offer when it was confirmed, so it stays remembered.
    expect(setNewProjectLocation).not.toHaveBeenCalled();
  });

  it('keeps a Local clicked while the lookup is pending once it lands', async () => {
    prefs.newProjectLocation = 'remote';
    const { lookup, calls } = deferredLookup();
    const { target, onConfirm } = await open(lookup);
    pickCreateRow(target, 'Fresh');
    expect(locationTile(target, 'Local')?.dataset.state).toBe('on');
    locationTile(target, 'Local')!.click();
    flushSync();

    calls[0].land(branchRepo);
    await settle();
    expect(locationTile(target, 'Remote')!.disabled).toBe(false);
    expect(locationTile(target, 'Local')?.dataset.state).toBe('on');
    expect(detectDefaultBranch).not.toHaveBeenCalled();

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'local');
    expect(setNewProjectLocation).toHaveBeenCalledExactlyOnceWith('local');
  });

  it('lets a branch note bound to no repo create a remote project with none', async () => {
    const { target, onConfirm } = await open(null);
    pickCreateRow(target, 'Fresh');
    chooseRemote(target);
    expect(locationTile(target, 'Remote')?.dataset.state).toBe('on');

    confirm(target);
    await vi.waitFor(() => expect(onConfirm).toHaveBeenCalledWith('created'));
    expect(createProject).toHaveBeenCalledWith('Fresh', 'remote');
  });

  it('says the lookup failed rather than that the note has no repo', async () => {
    const { lookup, calls } = deferredLookup();
    const { target, onConfirm } = await open(lookup);
    calls[0].fail(new Error('Note not found: note-1'));
    await settle();
    pickCreateRow(target, 'Fresh');
    expectRemoteBlocked(target, "Remote is unavailable: couldn't look up this note's repo");

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
    calls[0].fail(new Error('Note not found: note-1'));
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
    prefs.newProjectLocation = 'local';
    setNewProjectLocation.mockClear();
    store.deleting = new Set();
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

  it('remembers nothing when creating the project fails', async () => {
    createProject.mockRejectedValue(new Error('name taken'));
    const { target } = await open(branchRepo);
    pickCreateRow(target, 'Fresh');
    chooseRemote(target);

    confirm(target);
    await vi.waitFor(() => expect(errorText(target)).toBe('name taken'));
    expect(setNewProjectLocation).not.toHaveBeenCalled();
  });
});
