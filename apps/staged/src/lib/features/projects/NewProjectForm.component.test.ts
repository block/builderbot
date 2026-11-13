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
function open(selectedRepo: string | null = 'acme/widgets', onCancel?: () => void) {
  const target = document.createElement('div');
  document.body.appendChild(target);
  const onCreated = vi.fn();
  component = mount(NewProjectForm, {
    target,
    props: { onCreated, onCancel, name: 'Fresh', selectedRepo },
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

// Return creates the project through a form that owns only the Name field
// and Create: from the Name field the browser submits it, and a Return on
// the fields' container (where WebKit puts the focus after a click on one of
// its buttons) presses Create. There is no listener on the window, which
// used to create the project for a Return on a focused Cancel, and under any
// dialog opened over the splash screen.
describe('NewProjectForm Return key', () => {
  beforeEach(() => {
    createProject.mockReset().mockResolvedValue(created);
  });

  afterEach(() => {
    if (component) unmount(component);
    component = null;
    document.body.innerHTML = '';
  });

  function pressReturn(el: Element, init: KeyboardEventInit = {}): KeyboardEvent {
    const event = new KeyboardEvent('keydown', {
      key: 'Enter',
      bubbles: true,
      cancelable: true,
      ...init,
    });
    el.dispatchEvent(event);
    flushSync();
    return event;
  }

  const buttonNamed = (target: HTMLElement, name: string) =>
    [...target.querySelectorAll('button')].find((b) => b.textContent?.includes(name))!;
  const container = (target: HTMLElement) =>
    target.querySelector<HTMLElement>('.new-project-form')!;

  /** Put `el` where RepoConfigForm (stubbed out here) renders: after the Name field, before Create. */
  function amongTheRepoFields<T extends HTMLElement>(target: HTMLElement, el: T): T {
    target.querySelector('form')!.after(el);
    return el;
  }

  it('has a form that owns the Name field and Create, and nothing else', () => {
    const { target } = open('acme/widgets', vi.fn());
    const form = target.querySelector('form')!;

    expect([...form.elements]).toEqual([
      target.querySelector('#project-name'),
      buttonNamed(target, 'Create'),
    ]);
    expect(buttonNamed(target, 'Create').type).toBe('submit');
    expect(buttonNamed(target, 'Cancel').type).toBe('button');
  });

  it('creates the project when the form is submitted, as Return in the Name field does', async () => {
    const { target, onCreated } = open();

    target.querySelector('form')!.requestSubmit();

    await vi.waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(createProject).toHaveBeenCalledTimes(1);
  });

  it('leaves a Return in the Name field to the browser, which submits the form', () => {
    const { target } = open();

    const event = pressReturn(target.querySelector('#project-name')!);

    // Not pressed here as well: the browser's submit is the only one.
    expect(event.defaultPrevented).toBe(false);
    expect(createProject).not.toHaveBeenCalled();
  });

  it("creates the project on Return on the fields' container, which can take the focus", async () => {
    const { target, onCreated } = open();
    container(target).focus();
    expect(document.activeElement).toBe(container(target));

    const event = pressReturn(document.activeElement!);

    await vi.waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(createProject).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
  });

  it('presses Create, not a repo chip, which is a button with no type', async () => {
    const { target, onCreated } = open();
    const chip = amongTheRepoFields(target, document.createElement('button'));
    const chipClicked = vi.fn();
    chip.addEventListener('click', chipClicked);

    pressReturn(container(target));

    await vi.waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(chipClicked).not.toHaveBeenCalled();
  });

  it('does not create on a Return in a repo field, which is outside the form', () => {
    const { target } = open();
    const field = amongTheRepoFields(target, document.createElement('input'));

    const event = pressReturn(field);

    expect(field.form).toBeNull();
    expect(event.defaultPrevented).toBe(false);
    expect(createProject).not.toHaveBeenCalled();
  });

  it('does not create on a Return on the container that commits an IME composition, repeats or has a modifier', () => {
    const { target } = open();

    pressReturn(container(target), { keyCode: 229 });
    pressReturn(container(target), { repeat: true });
    pressReturn(container(target), { metaKey: true });

    expect(createProject).not.toHaveBeenCalled();
  });

  it('only cancels on Return on a focused Cancel', () => {
    const onCancel = vi.fn();
    const { target } = open('acme/widgets', onCancel);
    const cancel = buttonNamed(target, 'Cancel');
    cancel.focus();

    // The browser turns this keydown into Cancel's own click.
    const event = pressReturn(cancel);
    expect(event.defaultPrevented).toBe(false);
    expect(createProject).not.toHaveBeenCalled();

    cancel.click();
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(createProject).not.toHaveBeenCalled();
  });

  it('does not create on a Return outside the form', () => {
    open();

    pressReturn(document.body);

    expect(createProject).not.toHaveBeenCalled();
  });

  it('does not create while Create is disabled', () => {
    const target = document.createElement('div');
    document.body.appendChild(target);
    component = mount(NewProjectForm, { target, props: { onCreated: vi.fn(), name: '  ' } });
    flushSync();

    expect(buttonNamed(target, 'Create').disabled).toBe(true);
    buttonNamed(target, 'Create').click();
    const event = pressReturn(container(target));

    expect(createProject).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });
});
