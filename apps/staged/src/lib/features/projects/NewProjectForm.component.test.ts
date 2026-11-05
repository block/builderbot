import { flushSync, mount, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectLocation } from '../../types';

const { sq, prefs, setNewProjectLocation, createProject } = vi.hoisted(() => {
  const prefs = { newProjectLocation: 'local' as ProjectLocation };
  return {
    sq: { loaded: true, available: true },
    prefs,
    setNewProjectLocation: vi.fn((location: ProjectLocation) => {
      prefs.newProjectLocation = location;
    }),
    createProject: vi.fn(),
  };
});

vi.mock('./RepoConfigForm.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('../settings/sq.svelte', () => ({
  sqState: sq,
  bloxAvailable: () => sq.loaded && sq.available,
}));
vi.mock('../settings/preferences.svelte', () => ({ preferences: prefs, setNewProjectLocation }));
vi.mock('../../api/commands', () => ({ createProject, createProjectNote: vi.fn() }));

import NewProjectForm from './NewProjectForm.svelte';

const created = { id: 'created', name: 'Fresh' } as Project;

let component: ReturnType<typeof mount> | null = null;

/** Mount the form with a name typed and, unless `selectedRepo` is `null`, a repo picked. */
function open(selectedRepo: string | null = 'acme/widgets') {
  const target = document.createElement('div');
  document.body.appendChild(target);
  const onCreated = vi.fn();
  component = mount(NewProjectForm, {
    target,
    props: { onCreated, name: 'Fresh', selectedRepo },
  });
  flushSync();
  return { target, onCreated };
}

function locationTile(target: HTMLElement, label: 'Local' | 'Remote') {
  const picker = target.querySelector('[aria-label="Project location"]');
  return [...(picker?.querySelectorAll<HTMLButtonElement>('button') ?? [])].find((b) =>
    b.textContent?.includes(label)
  );
}

function create(target: HTMLElement) {
  [...target.querySelectorAll('button')].find((b) => b.textContent?.includes('Create'))!.click();
}

describe('NewProjectForm location', () => {
  beforeAll(() => {
    // jsdom has no Web Animations, which the error line's slide-in starts.
    Element.prototype.animate = () => ({ cancel: () => {} }) as unknown as Animation;
  });

  afterAll(() => {
    delete (Element.prototype as Partial<Element>).animate;
  });

  beforeEach(() => {
    sq.loaded = true;
    sq.available = true;
    prefs.newProjectLocation = 'local';
    setNewProjectLocation.mockClear();
    createProject.mockReset().mockResolvedValue(created);
  });

  afterEach(() => {
    if (component) unmount(component);
    component = null;
    document.body.innerHTML = '';
  });

  it('defaults to local and remembers it once the project is created', async () => {
    const { target, onCreated } = open();
    expect(locationTile(target, 'Local')?.dataset.state).toBe('on');

    create(target);
    await vi.waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(createProject.mock.calls[0].slice(0, 2)).toEqual(['Fresh', 'local']);
    expect(setNewProjectLocation).toHaveBeenCalledExactlyOnceWith('local');
  });

  it('starts on a remembered Remote and remembers a switch to Local only on create', async () => {
    prefs.newProjectLocation = 'remote';
    const { target, onCreated } = open();
    expect(locationTile(target, 'Remote')?.dataset.state).toBe('on');

    locationTile(target, 'Local')!.click();
    flushSync();
    expect(setNewProjectLocation).not.toHaveBeenCalled();

    create(target);
    await vi.waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(createProject.mock.calls[0].slice(0, 2)).toEqual(['Fresh', 'local']);
    expect(setNewProjectLocation).toHaveBeenCalledExactlyOnceWith('local');
  });

  it('creates remotely from a remembered Remote', async () => {
    prefs.newProjectLocation = 'remote';
    const { target, onCreated } = open();

    create(target);
    await vi.waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(createProject.mock.calls[0].slice(0, 3)).toEqual(['Fresh', 'remote', 'acme/widgets']);
    expect(setNewProjectLocation).toHaveBeenCalledExactlyOnceWith('remote');
  });

  it('creates a remote project with no repo, leaving its workspace for the first repo added', async () => {
    prefs.newProjectLocation = 'remote';
    const { target, onCreated } = open(null);
    const createButton = [...target.querySelectorAll('button')].find((b) =>
      b.textContent?.includes('Create')
    )!;
    expect(createButton.disabled).toBe(false);

    create(target);
    await vi.waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(createProject.mock.calls[0].slice(0, 3)).toEqual(['Fresh', 'remote', undefined]);
    expect(setNewProjectLocation).toHaveBeenCalledExactlyOnceWith('remote');
  });

  it('falls back to local without forgetting a remembered Remote when remote is unavailable', async () => {
    sq.available = false;
    prefs.newProjectLocation = 'remote';
    const { target, onCreated } = open();
    expect(target.querySelector('[aria-label="Project location"]')).toBeNull();

    create(target);
    await vi.waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(createProject.mock.calls[0].slice(0, 2)).toEqual(['Fresh', 'local']);
    expect(setNewProjectLocation).not.toHaveBeenCalled();
    expect(prefs.newProjectLocation).toBe('remote');
  });

  it('remembers nothing when creating the project fails', async () => {
    createProject.mockRejectedValue(new Error('name taken'));
    prefs.newProjectLocation = 'remote';
    const { target } = open();
    locationTile(target, 'Local')!.click();
    flushSync();

    create(target);
    await vi.waitFor(() => expect(target.textContent).toContain('name taken'));
    expect(setNewProjectLocation).not.toHaveBeenCalled();
  });
});
