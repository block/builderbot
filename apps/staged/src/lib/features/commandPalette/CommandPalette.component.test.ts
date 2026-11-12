import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch, Project, ProjectRepo } from '../../types';

// The palette under the real dialog primitive (focus scope, Escape layer,
// dismissible layer) and the real shortcut registry, with the stores it
// reads replaced by rune state the tests own (testStubs/paletteWorld), so a
// test can change the data while the palette is open. What is covered here
// is the wiring: keyboard and pointer selection, opening a result,
// open/close, focus, and what happens when the list changes under the
// highlight. The matching itself is covered in paletteSearch.test.ts.

const spies = vi.hoisted(() => ({
  selectProject: vi.fn(),
  ensureProjectsHydrated: vi.fn(async () => {}),
  ensureIndexLoaded: vi.fn(),
  toggleFilter: vi.fn(),
  clearFilters: vi.fn(),
  revealWithin: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock('../../stores/projectsData.svelte', async () => {
  const { world } = await import('./testStubs/paletteWorld.svelte');
  return {
    projectsDataStore: {
      get projects() {
        return world.projects;
      },
      get reposByProject() {
        return world.repos;
      },
      get branchesByProject() {
        return world.branches;
      },
      get loaded() {
        return world.loaded;
      },
      isProjectDeleting: (id: string) => world.deleting.has(id),
      isProjectHydrated: () => true,
      ensureProjectsHydrated: spies.ensureProjectsHydrated,
    },
  };
});
vi.mock('../layout/navigation.svelte', async () => {
  const { world } = await import('./testStubs/paletteWorld.svelte');
  return {
    navigation: {
      get selectedProjectId() {
        return world.selectedProjectId;
      },
      get currentRoute() {
        return world.currentRoute;
      },
      get homeActive() {
        return world.homeActive;
      },
      get projectLayer() {
        return world.projectLayer;
      },
    },
    selectProject: spies.selectProject,
    diffLeaveCheck: () => (world.currentRoute.kind === 'diff' ? world.diffLeaveCheck : null),
  };
});
vi.mock('../projects/projectFilters.svelte', async () => {
  const { world } = await import('./testStubs/paletteWorld.svelte');
  return {
    projectFiltersStore: {
      get hasActiveFilters() {
        return world.filtered !== null;
      },
      get filteredProjects() {
        return world.filtered ?? world.projects;
      },
      get activeFilters() {
        return world.activeFilters;
      },
      toggleFilter: spies.toggleFilter,
      clearFilters: spies.clearFilters,
    },
  };
});
vi.mock('../projects/projectSearchIndex.svelte', () => ({
  projectSearchIndex: { index: null, ensureLoaded: spies.ensureIndexLoaded },
}));
vi.mock('../projects/projectSearchContext', async () => {
  const { buildSearchSnapshot } = await import('../projects/projectSearch');
  const { world } = await import('./testStubs/paletteWorld.svelte');
  const repoBadgeShortName = (githubRepo: string, subpath: string | null) =>
    world.shortNames[subpath ? `${githubRepo}/${subpath}` : githubRepo];
  return {
    repoBadgeShortName,
    projectSearchSnapshots: () => () =>
      buildSearchSnapshot(world.projects, {
        branchesByProject: world.branches,
        reposByProject: world.repos,
        repoShortName: repoBadgeShortName,
      }),
  };
});
// The status icon reads preferences and PR state; not what these tests cover.
vi.mock(
  '../projects/ProjectStatusIcon.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('../../shared/revealWithin', () => ({ revealWithin: spies.revealWithin }));
vi.mock('svelte-sonner', () => ({ toast: { error: spies.toastError } }));

import { registerShortcuts } from '../keyboard/shortcuts';
import {
  captureGridStickyProject,
  projectsListViewState,
  requestProjectsListRestore,
} from '../projects/projectsListViewState.svelte';
import CommandPalette from './CommandPalette.svelte';
import {
  closeCommandPalette,
  commandPalette,
  commandPaletteShortcut,
  openCommandPalette,
  otherModalOpen,
} from './paletteState.svelte';
import { world } from './testStubs/paletteWorld.svelte';

// ── Fixtures ──

function project(id: string, name: string, updatedAt: number, githubRepo?: string): Project {
  return {
    id,
    name,
    githubRepo: githubRepo ?? null,
    location: 'local',
    subpath: null,
    createdAt: 0,
    updatedAt,
    statusOverride: null,
  };
}

/** A branch of `projectId` last touched at `updatedAt`: what hydration
 *  lands, and what moves a project up the recents. */
function branch(projectId: string, updatedAt: number): Branch {
  return {
    id: `${projectId}-branch`,
    projectId,
    projectRepoId: null,
    branchName: 'work',
    baseBranch: 'main',
    prNumber: null,
    prTitle: null,
    createdAt: 0,
    updatedAt,
  } as Branch;
}

function repo(projectId: string, overrides: Partial<ProjectRepo>): ProjectRepo {
  return {
    id: `${projectId}-repo`,
    projectId,
    githubRepo: 'org/repo',
    branchName: 'main',
    subpath: null,
    isPrimary: true,
    reason: null,
    headRepo: null,
    createdAt: 0,
    updatedAt: 0,
    ...overrides,
  };
}

// Most recently active first: Alpha, Beta, Gamma.
const alpha = project('alpha', 'Alpha service', 30, 'org/alpha');
const beta = project('beta', 'Beta tools', 20, 'org/beta');
const gamma = project('gamma', 'Gamma web', 10, 'org/gamma');

let component: ReturnType<typeof mount> | null = null;

/** Let the dialog's focus scope, presence and the re-render all land. */
async function settle() {
  // The dialog mounts, focuses and unmounts on animation frames.
  await new Promise((resolve) => requestAnimationFrame(resolve));
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
  }
}

async function openPalette() {
  expect(openCommandPalette()).toBe(true);
  await settle();
}

/** Change the stores' data the way the app does, between frames. */
async function change(mutate: () => void) {
  mutate();
  await settle();
}

const dialog = () => document.querySelector<HTMLElement>('[data-command-palette]');
/** The dialog leaves the DOM a frame or two after it closes. */
const dialogGone = () => vi.waitFor(() => expect(dialog()).toBeNull());
const input = () => document.querySelector<HTMLInputElement>('[data-command-palette] input')!;
const options = () => [
  ...document.querySelectorAll<HTMLElement>('[data-command-palette] [role="option"]'),
];
const optionIds = () => options().map((el) => el.dataset.projectId);
const optionNames = () => options().map((el) => el.querySelector('.row-name')?.textContent);
const optionFor = (id: string) => options().find((el) => el.dataset.projectId === id)!;
const selectedIds = () =>
  options()
    .filter((el) => el.getAttribute('aria-selected') === 'true')
    .map((el) => el.dataset.projectId);
const busyIds = () =>
  options()
    .filter((el) => el.getAttribute('aria-busy') === 'true')
    .map((el) => el.dataset.projectId);
const badges = () =>
  Object.fromEntries(
    options().map((el) => [
      el.dataset.projectId,
      [...el.querySelectorAll('.row-badges .palette-badge')].map((b) => b.textContent?.trim()),
    ])
  );
const liveRegion = () =>
  document.querySelector<HTMLElement>('[data-command-palette] [role="status"]');

/** A key pressed wherever focus is, as the keyboard delivers it. */
async function press(key: string, init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init });
  (document.activeElement ?? document.body).dispatchEvent(event);
  await settle();
  return event;
}

async function type(text: string) {
  const el = input();
  el.value = text;
  el.dispatchEvent(new Event('input', { bubbles: true }));
  await settle();
}

/**
 * A primary-button press on `el` as a browser delivers it: mousedown, whose
 * default action moves focus to the nearest focusable ancestor of the target
 * (or drops it), then mouseup and click. jsdom dispatches the events but
 * performs no default action, so the focus move is done here, and only when
 * the page didn't cancel the mousedown.
 */
async function pressOn(el: Element) {
  const down = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
  el.dispatchEvent(down);
  if (!down.defaultPrevented) {
    const focusable = el.closest<HTMLElement>('input, button, [tabindex]');
    if (focusable) focusable.focus();
    else (document.activeElement as HTMLElement | null)?.blur();
  }
  el.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
  el.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  await settle();
}

/**
 * The start of a primary-button press at a point of the page, which is what
 * the dialog dismisses on. jsdom lays nothing out (every box is empty at the
 * origin), so any point off the origin is outside the panel's box, as the
 * backdrop around it is in a browser.
 */
function pointerDownOn(el: Element) {
  el.dispatchEvent(
    new MouseEvent('pointerdown', {
      bubbles: true,
      cancelable: true,
      button: 0,
      clientX: 40,
      clientY: 40,
    })
  );
}

/** A focusable control in the page behind the palette. */
function pageControl<K extends 'button' | 'input'>(tag: K, parent: Element = document.body) {
  const el = document.createElement(tag);
  parent.appendChild(el);
  return el;
}

/** The host of the workspace layer on screen: where focus goes after a
 *  navigation (activeLayerHost). */
function workspaceLayer() {
  const layer = document.createElement('div');
  layer.className = 'workspace-layer';
  layer.tabIndex = -1;
  document.body.appendChild(layer);
  return layer;
}

beforeAll(() => {
  // jsdom lacks the layout APIs the dialog's viewport tracking uses.
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
});

beforeEach(() => {
  world.reset([gamma, alpha, beta]);
  projectsListViewState.searchQuery = '';
  projectsListViewState.returnTargetProjectId = null;
  projectsListViewState.restorePending = false;
  projectsListViewState.stickyProjectId = null;
  projectsListViewState.stickyFilters = null;
  for (const spy of Object.values(spies)) spy.mockReset();
  spies.ensureProjectsHydrated.mockResolvedValue(undefined);
  // The navigation the app's selectProject performs, as far as the palette
  // can see it.
  spies.selectProject.mockImplementation((id: string) => world.showProject(id));

  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(CommandPalette, { target });
  flushSync();
});

afterEach(async () => {
  closeCommandPalette();
  await settle();
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
  vi.restoreAllMocks();
});

describe('CommandPalette open and close', () => {
  it('renders nothing until opened', () => {
    expect(dialog()).toBeNull();
  });

  it('opens as a dialog with the search input focused and empty', async () => {
    await openPalette();

    expect(dialog()?.getAttribute('role')).toBe('dialog');
    expect(document.activeElement).toBe(input());
    expect(input().value).toBe('');
    // What a query will search is fetched on open.
    expect(spies.ensureProjectsHydrated).toHaveBeenCalled();
    expect(spies.ensureIndexLoaded).toHaveBeenCalled();
  });

  it('wires the input as a combobox over a listbox of options', async () => {
    await openPalette();

    const listbox = document.querySelector('[data-command-palette] [role="listbox"]')!;
    expect(input().getAttribute('role')).toBe('combobox');
    expect(input().getAttribute('aria-expanded')).toBe('true');
    expect(input().getAttribute('aria-controls')).toBe(listbox.id);
    expect(input().getAttribute('aria-activedescendant')).toBe(options()[0].id);
    expect(options().every((el) => el.id.startsWith(listbox.id))).toBe(true);
  });

  it('lists every project, most recently active first, with the first highlighted', async () => {
    await openPalette();

    expect(optionNames()).toEqual(['Alpha service', 'Beta tools', 'Gamma web']);
    expect(selectedIds()).toEqual(['alpha']);
  });

  it('closes on Escape without opening anything, returning focus', async () => {
    const before = pageControl('button');
    before.focus();
    await openPalette();
    expect(document.activeElement).toBe(input());

    await press('Escape');

    expect(commandPalette.open).toBe(false);
    await dialogGone();
    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(before);
  });

  it('leaves an Escape that cancels an IME composition to the IME', async () => {
    await openPalette();
    await type('ga');

    // While composing, and the committing key Safari reports after
    // compositionend (keyCode 229, isComposing already false).
    await press('Escape', { isComposing: true });
    await press('Escape', { keyCode: 229 });

    expect(commandPalette.open).toBe(true);
    expect(input().value).toBe('ga');
    expect(optionNames()).toEqual(['Gamma web']);

    await press('Escape');
    expect(commandPalette.open).toBe(false);
  });

  it('closes on a press on the backdrop without opening anything, returning focus', async () => {
    const before = pageControl('button');
    before.focus();
    await openPalette();
    const backdrop = document.querySelector<HTMLElement>('[data-slot="dialog-overlay"]')!;
    expect(backdrop).not.toBeNull();

    pointerDownOn(backdrop);

    await vi.waitFor(() => expect(commandPalette.open).toBe(false));
    await dialogGone();
    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(before);
  });

  it('stays open on a press inside the panel', async () => {
    await openPalette();

    pointerDownOn(dialog()!.querySelector('.palette-footer')!);
    // Past the dismissible layer's debounce.
    await new Promise((resolve) => setTimeout(resolve, 30));
    await settle();

    expect(commandPalette.open).toBe(true);
    expect(dialog()).not.toBeNull();
  });

  it('starts from an empty query and the first row on every open', async () => {
    await openPalette();
    await type('gam');
    expect(optionNames()).toEqual(['Gamma web']);
    closeCommandPalette();
    await settle();

    await openPalette();

    expect(input().value).toBe('');
    expect(optionNames()).toEqual(['Alpha service', 'Beta tools', 'Gamma web']);
    expect(selectedIds()).toEqual(['alpha']);
  });

  it('starts from the first row again after a highlight further down', async () => {
    await openPalette();
    await press('ArrowDown');
    await press('ArrowDown');
    expect(selectedIds()).toEqual(['gamma']);
    closeCommandPalette();
    await settle();

    await openPalette();

    expect(selectedIds()).toEqual(['alpha']);
  });
});

describe('the ⌘K shortcut', () => {
  let unregister: () => void;
  let enabled: boolean;

  beforeEach(() => {
    enabled = true;
    // The definition App.svelte registers, through the registry it uses.
    unregister = registerShortcuts([commandPaletteShortcut(() => enabled)]);
  });

  afterEach(() => unregister());

  /** Ctrl+K off macOS (jsdom's platform), ⌘K on it. */
  async function shortcut(target: EventTarget, init: KeyboardEventInit = { ctrlKey: true }) {
    const event = new KeyboardEvent('keydown', {
      key: 'k',
      bubbles: true,
      cancelable: true,
      ...init,
    });
    target.dispatchEvent(event);
    await settle();
    return event;
  }

  function addModal(attributes: Record<string, string>) {
    const modal = document.createElement('div');
    for (const [name, value] of Object.entries(attributes)) modal.setAttribute(name, value);
    document.body.appendChild(modal);
    return modal;
  }

  it('opens the palette from a focused text field and takes the key', async () => {
    const field = pageControl('input');
    field.focus();

    const event = await shortcut(field);

    expect(dialog()).not.toBeNull();
    expect(document.activeElement).toBe(input());
    expect(event.defaultPrevented).toBe(true);
  });

  it("closes it again from the palette's own input, returning focus to the field", async () => {
    const field = pageControl('input');
    field.focus();
    await shortcut(field);
    expect(document.activeElement).toBe(input());
    await type('gam');

    const event = await shortcut(input());

    expect(commandPalette.open).toBe(false);
    await dialogGone();
    expect(event.defaultPrevented).toBe(true);
    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(field);
  });

  it('is ⌘K on macOS, where Ctrl+K is left alone', async () => {
    vi.spyOn(window.navigator, 'platform', 'get').mockReturnValue('MacIntel');

    const ctrl = await shortcut(document.body, { ctrlKey: true });
    expect(commandPalette.open).toBe(false);
    expect(ctrl.defaultPrevented).toBe(false);

    const meta = await shortcut(document.body, { metaKey: true });
    expect(dialog()).not.toBeNull();
    expect(meta.defaultPrevented).toBe(true);
  });

  it('needs the modifier: a plain k is typing', async () => {
    const event = await shortcut(document.body, {});

    expect(commandPalette.open).toBe(false);
    expect(event.defaultPrevented).toBe(false);
  });

  it('opens once while the key is held, taking the repeats', async () => {
    await shortcut(document.body);
    expect(commandPalette.open).toBe(true);

    for (let i = 0; i < 3; i++) {
      const repeat = await shortcut(input(), { ctrlKey: true, repeat: true });
      expect(repeat.defaultPrevented).toBe(true);
      expect(commandPalette.open).toBe(true);
    }
    expect(dialog()).not.toBeNull();

    // The next real press closes it.
    await shortcut(input());
    expect(commandPalette.open).toBe(false);
  });

  it('does not open while another dialog is open, and still takes the key', async () => {
    addModal({ role: 'dialog', 'data-state': 'open' });
    expect(otherModalOpen()).toBe(true);

    const event = await shortcut(document.body);

    expect(commandPalette.open).toBe(false);
    expect(dialog()).toBeNull();
    expect(event.defaultPrevented).toBe(true);
  });

  it('does not open over a pending confirmation', async () => {
    addModal({ role: 'alertdialog', 'data-state': 'open' });

    const event = await shortcut(document.body);

    expect(commandPalette.open).toBe(false);
    expect(event.defaultPrevented).toBe(true);
  });

  it('ignores a dialog that is only animating out', async () => {
    addModal({ role: 'dialog', 'data-state': 'closed' });
    expect(otherModalOpen()).toBe(false);

    await shortcut(document.body);

    expect(dialog()).not.toBeNull();
  });

  it('does not count itself as another dialog', async () => {
    await shortcut(document.body);

    expect(otherModalOpen()).toBe(false);
  });

  it('does nothing where there is no workspace to search, and still takes the key', async () => {
    enabled = false;

    const event = await shortcut(document.body);

    expect(commandPalette.open).toBe(false);
    expect(event.defaultPrevented).toBe(true);
  });
});

describe('CommandPalette keyboard navigation', () => {
  it('moves the highlight with the arrow keys', async () => {
    await openPalette();

    await press('ArrowDown');
    expect(selectedIds()).toEqual(['beta']);
    expect(input().getAttribute('aria-activedescendant')).toBe(options()[1].id);

    await press('ArrowDown');
    expect(selectedIds()).toEqual(['gamma']);

    await press('ArrowUp');
    expect(selectedIds()).toEqual(['beta']);
  });

  it('wraps at both ends', async () => {
    await openPalette();

    await press('ArrowUp');
    expect(selectedIds()).toEqual(['gamma']);

    await press('ArrowDown');
    expect(selectedIds()).toEqual(['alpha']);
  });

  it('scrolls the highlighted row into view within the list', async () => {
    await openPalette();
    spies.revealWithin.mockClear();

    await press('ArrowDown');

    const listbox = document.querySelector('[data-command-palette] [role="listbox"]');
    expect(spies.revealWithin).toHaveBeenCalledTimes(1);
    expect(spies.revealWithin).toHaveBeenCalledWith(options()[1], listbox, { block: 'nearest' });
  });

  it('keeps focus in the input while the highlight moves', async () => {
    await openPalette();

    await press('ArrowDown');

    expect(document.activeElement).toBe(input());
  });

  it('opens the highlighted project on Enter and closes', async () => {
    await openPalette();
    await press('ArrowDown');

    await press('Enter');

    expect(spies.selectProject).toHaveBeenCalledTimes(1);
    expect(spies.selectProject).toHaveBeenCalledWith('beta');
    expect(commandPalette.open).toBe(false);
    await dialogGone();
  });

  it('keeps Enter and the arrows from the views listening under the dialog', async () => {
    const underneath = vi.fn();
    window.addEventListener('keydown', underneath);
    await openPalette();

    const down = await press('ArrowDown');
    const enter = await press('Enter');
    window.removeEventListener('keydown', underneath);

    expect(underneath).not.toHaveBeenCalled();
    expect(down.defaultPrevented).toBe(true);
    expect(enter.defaultPrevented).toBe(true);
  });

  it('leaves modified and composing keys alone', async () => {
    await openPalette();

    await press('ArrowDown', { metaKey: true });
    await press('Enter', { isComposing: true });

    expect(selectedIds()).toEqual(['alpha']);
    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(commandPalette.open).toBe(true);
  });

  it('only closes when the highlighted project is already on screen', async () => {
    world.showProject('alpha');
    const before = pageControl('button', workspaceLayer());
    before.focus();
    await openPalette();
    // The project on screen is listed last.
    expect(optionNames()).toEqual(['Beta tools', 'Gamma web', 'Alpha service']);
    await press('ArrowUp');
    expect(selectedIds()).toEqual(['alpha']);

    await press('Enter');

    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(commandPalette.open).toBe(false);
    // Nothing changed behind the palette: focus goes back where it was.
    await dialogGone();
    expect(document.activeElement).toBe(before);
  });
});

describe('CommandPalette focus', () => {
  it('moves focus to the view that came up after opening a project', async () => {
    world.showProject('alpha');
    const layer = workspaceLayer();
    // A control of the view being left; it would be the default place for
    // focus to return to.
    const before = pageControl('button', layer);
    before.focus();
    await openPalette();

    await press('Enter');

    expect(spies.selectProject).toHaveBeenCalledWith('beta');
    await dialogGone();
    expect(document.activeElement).toBe(layer);
  });

  it('does the same when the project is opened with a click', async () => {
    const layer = workspaceLayer();
    const before = pageControl('button');
    before.focus();
    await openPalette();

    await pressOn(optionFor('gamma'));

    expect(spies.selectProject).toHaveBeenCalledWith('gamma');
    await dialogGone();
    expect(document.activeElement).toBe(layer);
  });

  it('keeps focus in the field through a press on a row that cannot be opened', async () => {
    world.deleting = new Set(['beta']);
    await openPalette();

    await pressOn(optionFor('beta'));

    expect(commandPalette.open).toBe(true);
    expect(document.activeElement).toBe(input());
  });

  // The dialog itself is focusable (the focus trap's fallback), so a press
  // on anything in the panel that isn't the field would move focus there
  // and leave the field dead.
  it.each([
    ['the footer hints', '.palette-footer .hint'],
    ['the result count', '.palette-footer .count'],
    ['the search icon', '.palette-search svg'],
    ["the list's padding", '[role="listbox"]'],
  ])('keeps focus in the field through a press on %s', async (_what, selector) => {
    await openPalette();
    const target = dialog()!.querySelector(selector)!;
    expect(target).not.toBeNull();

    await pressOn(target);

    expect(commandPalette.open).toBe(true);
    expect(document.activeElement).toBe(input());
    // The keyboard still drives the list, and typing still filters it.
    await press('ArrowDown');
    expect(selectedIds()).toEqual(['beta']);
    await type('gam');
    expect(optionNames()).toEqual(['Gamma web']);
  });

  it('keeps focus in the field through a press on the empty state', async () => {
    await openPalette();
    await type('zzzz');

    await pressOn(dialog()!.querySelector('.palette-empty .empty-hint')!);

    expect(document.activeElement).toBe(input());
    await type('beta');
    expect(optionNames()).toEqual(['Beta tools']);
  });

  it('still lets a press on the field itself place the caret', async () => {
    await openPalette();

    const down = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    input().dispatchEvent(down);

    expect(down.defaultPrevented).toBe(false);
  });

  it('takes the keys, and focus back to the field, when the dialog itself has focus', async () => {
    const underneath = vi.fn();
    window.addEventListener('keydown', underneath);
    await openPalette();
    dialog()!.focus();
    expect(document.activeElement).toBe(dialog());

    await press('ArrowDown');

    expect(selectedIds()).toEqual(['beta']);
    expect(document.activeElement).toBe(input());

    dialog()!.focus();
    await press('Enter');
    window.removeEventListener('keydown', underneath);

    expect(spies.selectProject).toHaveBeenCalledWith('beta');
    expect(underneath).not.toHaveBeenCalled();
  });

  it('sends a typed character to the field when the dialog itself has focus', async () => {
    await openPalette();
    dialog()!.focus();

    await press('g');

    // The keydown moved focus, so the character it types lands in the field.
    expect(document.activeElement).toBe(input());
  });
});

describe('CommandPalette filtering', () => {
  it('filters as the query is typed and highlights the match', async () => {
    await openPalette();

    await type('beta');

    expect(optionNames()).toEqual(['Beta tools']);
    expect(options()[0].querySelector('.row-name mark')?.textContent).toBe('Beta');
  });

  it('returns the highlight to the first result when the query changes', async () => {
    await openPalette();
    await press('ArrowDown');
    await press('ArrowDown');
    expect(selectedIds()).toEqual(['gamma']);

    await type('a');

    expect(optionNames()).toEqual(['Alpha service', 'Beta tools', 'Gamma web']);
    expect(selectedIds()).toEqual(['alpha']);
  });

  it('opens the first result of a typed query on Enter', async () => {
    await openPalette();
    await type('org/gamma');

    await press('Enter');

    expect(spies.selectProject).toHaveBeenCalledWith('gamma');
  });

  it('says so when nothing matches, and Enter does nothing', async () => {
    await openPalette();

    await type('zzzz');

    expect(options()).toEqual([]);
    expect(dialog()!.querySelector('.palette-empty')?.textContent).toContain(
      'No projects match “zzzz”'
    );
    expect(input().getAttribute('aria-expanded')).toBe('false');
    expect(input().hasAttribute('aria-activedescendant')).toBe(false);

    await press('Enter');
    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(commandPalette.open).toBe(true);
  });

  it('shows the keyboard hints', async () => {
    await openPalette();

    const footer = document.querySelector('[data-command-palette] .palette-footer')!;
    expect(footer.textContent?.replace(/\s+/g, ' ')).toContain('↑↓ navigate ↵ open esc close');
    expect(footer.textContent).toContain('3 projects');
  });

  it('shows the label a project was found by when the repo path does not say', async () => {
    world.repos = new Map([
      ['beta', [repo('beta', { githubRepo: 'org/payments', headRepo: 'matt/pay-fork' })]],
      ['gamma', [repo('gamma', { githubRepo: 'org/web', subpath: 'apps/site' })]],
    ]);
    world.shortNames = { 'org/web/apps/site': 'storefront' };
    await openPalette();
    // Unsearched, a fork PR's head repo stands in for the base.
    expect(optionFor('beta').querySelector('.row-repo')?.textContent).toBe('matt/pay-fork');

    await type('payments');
    expect(optionIds()).toEqual(['beta']);
    expect(optionFor('beta').querySelector('.row-repo')?.textContent).toBe('org/payments');
    expect(optionFor('beta').querySelector('.row-repo mark')?.textContent).toBe('payments');

    await type('storefront');
    expect(optionIds()).toEqual(['gamma']);
    expect(optionFor('gamma').querySelector('.row-alias mark')?.textContent).toBe('storefront');
    expect(optionFor('gamma').querySelector('.row-repo')?.textContent).toBe('org/web/apps/site');
  });
});

describe('CommandPalette announcements', () => {
  it('announces the result count and the empty state from one live region', async () => {
    await openPalette();
    const region = liveRegion()!;
    expect(region.getAttribute('aria-live')).toBe('polite');
    expect(region.textContent).toBe('3 projects');

    await type('zzzz');
    // The same element, with new text: a region that is replaced, or
    // inserted already holding its text, is often not announced.
    expect(liveRegion()).toBe(region);
    expect(region.textContent).toBe('No projects match “zzzz”');

    await type('gam');
    expect(liveRegion()).toBe(region);
    expect(region.textContent).toBe('1 project');
    expect(document.querySelectorAll('[data-command-palette] [aria-live]')).toHaveLength(1);
  });

  it('says when there is nothing to list yet', async () => {
    world.projects = [];
    world.loaded = false;
    await openPalette();
    expect(liveRegion()!.textContent).toBe('Loading projects…');

    await change(() => (world.loaded = true));
    expect(liveRegion()!.textContent).toBe('No projects yet');

    await change(() => (world.projects = [alpha]));
    expect(liveRegion()!.textContent).toBe('1 project');
  });
});

describe('CommandPalette pointer', () => {
  it('moves the highlight to the row under the pointer', async () => {
    await openPalette();
    spies.revealWithin.mockClear();

    options()[2].dispatchEvent(new MouseEvent('pointermove', { bubbles: true }));
    await settle();

    expect(selectedIds()).toEqual(['gamma']);
    // Hovering doesn't scroll the list out from under the pointer.
    expect(spies.revealWithin).not.toHaveBeenCalled();
  });

  it('opens the clicked project and closes', async () => {
    await openPalette();

    await pressOn(options()[1]);

    expect(spies.selectProject).toHaveBeenCalledWith('beta');
    expect(commandPalette.open).toBe(false);
  });

  it('keeps focus in the field when a row is pressed without being opened', async () => {
    await openPalette();

    // The press without the release: a drag off the row, or the moment
    // before the click lands.
    const down = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    options()[1].dispatchEvent(down);
    if (!down.defaultPrevented) options()[1].focus();
    await settle();

    expect(document.activeElement).toBe(input());
    expect(commandPalette.open).toBe(true);
    await type('gam');
    expect(optionNames()).toEqual(['Gamma web']);
  });
});

// The list changes while the palette is open: opening it fetches every
// project's branches, and each landing can reorder the recents; projects
// are created, renamed and removed by sessions and other windows.
describe('CommandPalette when the projects change while it is open', () => {
  it('keeps the highlight on its project when the list reorders, and Enter opens it', async () => {
    await openPalette();
    await press('ArrowDown');
    expect(selectedIds()).toEqual(['beta']);

    // Gamma's branches land: it is the most recently active after all.
    await change(() => (world.branches = new Map([['gamma', [branch('gamma', 99)]]])));

    expect(optionIds()).toEqual(['gamma', 'alpha', 'beta']);
    expect(selectedIds()).toEqual(['beta']);
    expect(input().getAttribute('aria-activedescendant')).toBe(optionFor('beta').id);

    await press('Enter');
    expect(spies.selectProject).toHaveBeenCalledTimes(1);
    expect(spies.selectProject).toHaveBeenCalledWith('beta');
  });

  it('leaves a highlight that was never moved on the top row when another moves above it', async () => {
    await openPalette();
    expect(selectedIds()).toEqual(['alpha']);
    spies.revealWithin.mockClear();

    // Nothing was pressed: the highlight is the default, not a choice of
    // Alpha, so it is the most recent project's, whichever that turns out
    // to be.
    await change(() => (world.branches = new Map([['gamma', [branch('gamma', 99)]]])));

    expect(optionIds()).toEqual(['gamma', 'alpha', 'beta']);
    expect(selectedIds()).toEqual(['gamma']);
    expect(input().getAttribute('aria-activedescendant')).toBe(optionFor('gamma').id);
    // And the list isn't scrolled after the row that used to be on top.
    expect(spies.revealWithin).not.toHaveBeenCalled();

    await press('Enter');
    expect(spies.selectProject).toHaveBeenCalledTimes(1);
    expect(spies.selectProject).toHaveBeenCalledWith('gamma');
  });

  it('keeps the first row’s project highlighted when another moves above it, once the arrows chose it', async () => {
    await openPalette();
    await press('ArrowDown');
    await press('ArrowUp');
    expect(selectedIds()).toEqual(['alpha']);

    await change(() => (world.branches = new Map([['gamma', [branch('gamma', 99)]]])));

    expect(optionIds()).toEqual(['gamma', 'alpha', 'beta']);
    expect(selectedIds()).toEqual(['alpha']);

    await press('Enter');
    expect(spies.selectProject).toHaveBeenCalledWith('alpha');
  });

  it('keeps the highlight on the project the pointer chose when the list reorders', async () => {
    await openPalette();
    options()[1].dispatchEvent(new MouseEvent('pointermove', { bubbles: true }));
    await settle();
    expect(selectedIds()).toEqual(['beta']);

    await change(() => (world.branches = new Map([['gamma', [branch('gamma', 99)]]])));

    expect(optionIds()).toEqual(['gamma', 'alpha', 'beta']);
    expect(selectedIds()).toEqual(['beta']);
  });

  it('keeps the highlight through a reorder under a typed query', async () => {
    await openPalette();
    await type('a');
    await press('ArrowDown');
    expect(selectedIds()).toEqual(['beta']);

    await change(() => (world.branches = new Map([['gamma', [branch('gamma', 99)]]])));

    expect(optionIds()).toEqual(['gamma', 'alpha', 'beta']);
    expect(selectedIds()).toEqual(['beta']);
  });

  it('moves the arrow keys on from where the highlighted project now is', async () => {
    await openPalette();
    await press('ArrowDown');
    await change(() => (world.branches = new Map([['gamma', [branch('gamma', 99)]]])));
    expect(optionIds()).toEqual(['gamma', 'alpha', 'beta']);

    await press('ArrowUp');

    expect(selectedIds()).toEqual(['alpha']);
  });

  it('brings the highlighted row back into view when a reorder moves it', async () => {
    await openPalette();
    await press('ArrowDown');
    spies.revealWithin.mockClear();

    await change(() => (world.branches = new Map([['gamma', [branch('gamma', 99)]]])));

    const listbox = document.querySelector('[data-command-palette] [role="listbox"]');
    expect(spies.revealWithin).toHaveBeenCalledWith(optionFor('beta'), listbox, {
      block: 'nearest',
    });
  });

  it('keeps the highlight when a project is created above it', async () => {
    await openPalette();
    await press('ArrowDown');

    await change(() => (world.projects = [...world.projects, project('delta', 'Delta api', 99)]));

    expect(optionIds()).toEqual(['delta', 'alpha', 'beta', 'gamma']);
    expect(selectedIds()).toEqual(['beta']);

    await press('Enter');
    expect(spies.selectProject).toHaveBeenCalledWith('beta');
  });

  it('keeps the highlight when a project above it is removed', async () => {
    await openPalette();
    await press('ArrowDown');
    await press('ArrowDown');
    expect(selectedIds()).toEqual(['gamma']);

    await change(() => (world.projects = world.projects.filter((p) => p.id !== 'alpha')));

    expect(optionIds()).toEqual(['beta', 'gamma']);
    expect(selectedIds()).toEqual(['gamma']);

    await press('Enter');
    expect(spies.selectProject).toHaveBeenCalledWith('gamma');
  });

  it('moves the highlight to the row that took its place when its project is removed', async () => {
    await openPalette();
    await press('ArrowDown');
    expect(selectedIds()).toEqual(['beta']);

    await change(() => (world.projects = world.projects.filter((p) => p.id !== 'beta')));

    expect(optionIds()).toEqual(['alpha', 'gamma']);
    expect(selectedIds()).toEqual(['gamma']);
    expect(input().getAttribute('aria-activedescendant')).toBe(optionFor('gamma').id);

    // And it is that project's now: a later reorder doesn't hand the
    // highlight to whatever lands on the row.
    await change(() => (world.branches = new Map([['gamma', [branch('gamma', 99)]]])));
    expect(optionIds()).toEqual(['gamma', 'alpha']);
    expect(selectedIds()).toEqual(['gamma']);

    await press('Enter');
    expect(spies.selectProject).toHaveBeenCalledWith('gamma');
  });

  it('moves the highlight to the new last row when the last project is removed', async () => {
    await openPalette();
    await press('ArrowUp');
    expect(selectedIds()).toEqual(['gamma']);

    await change(() => (world.projects = world.projects.filter((p) => p.id !== 'gamma')));

    expect(optionIds()).toEqual(['alpha', 'beta']);
    expect(selectedIds()).toEqual(['beta']);
  });

  it('has nothing highlighted or openable once every project is gone, then recovers', async () => {
    await openPalette();
    await press('ArrowDown');

    await change(() => (world.projects = []));

    expect(options()).toEqual([]);
    expect(input().hasAttribute('aria-activedescendant')).toBe(false);
    await press('Enter');
    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(commandPalette.open).toBe(true);

    await change(() => (world.projects = [gamma, alpha]));
    expect(optionIds()).toEqual(['alpha', 'gamma']);
    expect(selectedIds()).toHaveLength(1);
  });

  it('shows a rename in the highlighted row and still opens that project', async () => {
    await openPalette();
    await press('ArrowDown');

    await change(
      () =>
        (world.projects = world.projects.map((p) =>
          p.id === 'beta' ? { ...p, name: 'Billing tools' } : p
        ))
    );

    expect(optionNames()).toEqual(['Alpha service', 'Billing tools', 'Gamma web']);
    expect(selectedIds()).toEqual(['beta']);

    await press('Enter');
    expect(spies.selectProject).toHaveBeenCalledWith('beta');
  });

  it('highlights a better match that arrives above the first result of a query', async () => {
    await openPalette();
    await type('tools');
    expect(optionIds()).toEqual(['beta']);
    expect(selectedIds()).toEqual(['beta']);
    spies.revealWithin.mockClear();

    // Gamma is renamed into the results, and outranks Beta on recency. The
    // highlight was never moved, so it is still the top result's.
    await change(
      () =>
        (world.projects = world.projects.map((p) =>
          p.id === 'gamma' ? { ...p, name: 'Tools web', updatedAt: 99 } : p
        ))
    );

    expect(optionIds()).toEqual(['gamma', 'beta']);
    expect(selectedIds()).toEqual(['gamma']);
    expect(spies.revealWithin).not.toHaveBeenCalled();

    await press('Enter');
    expect(spies.selectProject).toHaveBeenCalledWith('gamma');
  });

  it('drops a project renamed out of the query, and Enter opens what is highlighted', async () => {
    await openPalette();
    await type('a');
    await press('ArrowDown');
    expect(selectedIds()).toEqual(['beta']);

    await change(
      () =>
        (world.projects = world.projects.map((p) =>
          p.id === 'beta' ? { ...p, name: 'Tools' } : p
        ))
    );

    // "Tools" has no "a" in its name; its repo (org/beta) still does.
    expect(optionIds()).toEqual(['alpha', 'gamma', 'beta']);
    expect(selectedIds()).toEqual(['beta']);

    await change(
      () =>
        (world.projects = world.projects.map((p) =>
          p.id === 'beta' ? { ...p, githubRepo: 'org/tools' } : p
        ))
    );

    expect(optionIds()).toEqual(['alpha', 'gamma']);
    const [highlighted] = selectedIds();
    expect(highlighted).toBe('gamma');
    await press('Enter');
    expect(spies.selectProject).toHaveBeenCalledWith('gamma');
  });

  it('marks the highlighted project as it starts being removed, and stops opening it', async () => {
    await openPalette();
    await press('ArrowDown');
    expect(badges().beta).toEqual([]);

    await change(() => (world.deleting = new Set(['beta'])));

    // Listed, as in the sidebar until the backend confirms, but disabled.
    expect(optionIds()).toEqual(['alpha', 'beta', 'gamma']);
    expect(selectedIds()).toEqual(['beta']);
    expect(optionFor('beta').getAttribute('aria-disabled')).toBe('true');
    expect(badges().beta).toEqual(['Removing…']);

    await press('Enter');
    await pressOn(optionFor('beta'));
    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(commandPalette.open).toBe(true);

    // The removal lands: the highlight moves to the row that took its place.
    await change(() => {
      world.projects = world.projects.filter((p) => p.id !== 'beta');
      world.deleting = new Set();
    });
    expect(selectedIds()).toEqual(['gamma']);
    await press('Enter');
    expect(spies.selectProject).toHaveBeenCalledWith('gamma');
  });

  it('opens a project again once its removal is called off', async () => {
    world.deleting = new Set(['alpha']);
    await openPalette();
    await press('Enter');
    expect(spies.selectProject).not.toHaveBeenCalled();

    await change(() => (world.deleting = new Set()));

    expect(badges().alpha).toEqual([]);
    await press('Enter');
    expect(spies.selectProject).toHaveBeenCalledWith('alpha');
  });

  it('updates the badges as the project on screen and the filters change', async () => {
    world.showProject('alpha');
    await openPalette();
    expect(badges()).toEqual({ beta: [], gamma: [], alpha: ['Current'] });

    await change(() => {
      world.filtered = [gamma];
      world.activeFilters = new Set(['unread']);
    });

    expect(badges()).toEqual({ beta: ['Filtered out'], gamma: [], alpha: ['Current'] });
  });
});

describe('CommandPalette over Settings and the diff view', () => {
  it('opens a project from Settings like any other', async () => {
    world.showOverProject('settings', 'alpha');
    await openPalette();

    await press('Enter');

    expect(spies.selectProject).toHaveBeenCalledWith('beta');
    expect(commandPalette.open).toBe(false);
  });

  it('opens the project under Settings rather than only closing', async () => {
    world.showOverProject('settings', 'alpha');
    await openPalette();
    await press('ArrowUp');
    expect(selectedIds()).toEqual(['alpha']);

    await press('Enter');

    // Settings is what is on screen, so the project is not: selectProject
    // closes Settings onto it.
    expect(spies.selectProject).toHaveBeenCalledWith('alpha');
  });

  it('waits for the diff view to save what was typed into it before leaving', async () => {
    world.showOverProject('diff', 'alpha');
    let finishSave!: (saved: boolean) => void;
    world.diffLeaveCheck = vi.fn(() => new Promise<boolean>((resolve) => (finishSave = resolve)));
    await openPalette();

    await press('Enter');

    expect(world.diffLeaveCheck).toHaveBeenCalledTimes(1);
    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(commandPalette.open).toBe(true);

    finishSave(true);
    await settle();

    expect(spies.selectProject).toHaveBeenCalledTimes(1);
    expect(spies.selectProject).toHaveBeenCalledWith('beta');
    expect(commandPalette.open).toBe(false);
  });

  it('shows the wait on the row being opened while the diff view saves', async () => {
    world.showOverProject('diff', 'alpha');
    let finishSave!: (saved: boolean) => void;
    world.diffLeaveCheck = vi.fn(() => new Promise<boolean>((resolve) => (finishSave = resolve)));
    await openPalette();
    expect(busyIds()).toEqual([]);
    expect(dialog()!.querySelector('.row-busy')).toBeNull();

    await press('Enter');

    expect(busyIds()).toEqual(['beta']);
    const wait = optionFor('beta').querySelector('.row-busy');
    expect(wait?.textContent?.trim()).toBe('Saving comment…');
    expect(wait?.querySelector('.spinner')).not.toBeNull();
    expect(liveRegion()?.textContent).toBe('Saving comment…');

    // It is the project being opened that waits, wherever the highlight goes.
    await press('ArrowDown');
    expect(selectedIds()).toEqual(['gamma']);
    expect(busyIds()).toEqual(['beta']);

    finishSave(true);
    await settle();

    expect(spies.selectProject).toHaveBeenCalledWith('beta');
    expect(commandPalette.open).toBe(false);
  });

  it('closes on Escape while the save runs, and the next open shows no wait', async () => {
    world.showOverProject('diff', 'alpha');
    let finishSave!: (saved: boolean) => void;
    world.diffLeaveCheck = vi.fn(() => new Promise<boolean>((resolve) => (finishSave = resolve)));
    await openPalette();
    await press('Enter');
    expect(busyIds()).toEqual(['beta']);

    await press('Escape');

    expect(commandPalette.open).toBe(false);
    await dialogGone();

    await openPalette();
    expect(busyIds()).toEqual([]);
    expect(liveRegion()?.textContent).toBe('3 projects');

    // The abandoned save landing changes nothing for this open.
    finishSave(true);
    await settle();
    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(busyIds()).toEqual([]);
  });

  it('stays on the diff and says so when the comment did not save', async () => {
    world.showOverProject('diff', 'alpha');
    world.diffLeaveCheck = vi.fn(async () => false);
    const editor = pageControl('input');
    editor.focus();
    await openPalette();

    await press('Enter');

    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(world.currentRoute.kind).toBe('diff');
    expect(spies.toastError).toHaveBeenCalledTimes(1);
    expect(spies.toastError.mock.calls[0][0]).toBe('Comment not saved');
    // The palette gets out of the way, and focus returns to the comment.
    expect(commandPalette.open).toBe(false);
    await dialogGone();
    expect(document.activeElement).toBe(editor);
  });

  it('stays on the diff when the save throws', async () => {
    world.showOverProject('diff', 'alpha');
    world.diffLeaveCheck = vi.fn(async () => {
      throw new Error('offline');
    });
    vi.spyOn(console, 'error').mockImplementation(() => {});
    await openPalette();

    await press('Enter');

    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(spies.toastError).toHaveBeenCalledTimes(1);
  });

  it('goes nowhere when the palette is dismissed while the save runs', async () => {
    world.showOverProject('diff', 'alpha');
    let finishSave!: (saved: boolean) => void;
    world.diffLeaveCheck = vi.fn(() => new Promise<boolean>((resolve) => (finishSave = resolve)));
    await openPalette();
    await press('Enter');

    await press('Escape');
    finishSave(true);
    await settle();

    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(spies.toastError).not.toHaveBeenCalled();
  });

  it('goes nowhere when the palette was reopened while the save ran', async () => {
    world.showOverProject('diff', 'alpha');
    let finishSave!: (saved: boolean) => void;
    world.diffLeaveCheck = vi.fn(() => new Promise<boolean>((resolve) => (finishSave = resolve)));
    await openPalette();
    await press('Enter');
    await press('Escape');
    await dialogGone();
    await openPalette();

    finishSave(true);
    await settle();

    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(commandPalette.open).toBe(true);
  });

  it('asks the diff view once however often Enter is pressed meanwhile', async () => {
    world.showOverProject('diff', 'alpha');
    let finishSave!: (saved: boolean) => void;
    world.diffLeaveCheck = vi.fn(() => new Promise<boolean>((resolve) => (finishSave = resolve)));
    await openPalette();

    await press('Enter');
    await press('Enter');
    await pressOn(optionFor('gamma'));
    finishSave(true);
    await settle();

    expect(world.diffLeaveCheck).toHaveBeenCalledTimes(1);
    expect(spies.selectProject).toHaveBeenCalledTimes(1);
    expect(spies.selectProject).toHaveBeenCalledWith('beta');
  });

  it('does not open a project that started being removed while the save ran', async () => {
    world.showOverProject('diff', 'alpha');
    let finishSave!: (saved: boolean) => void;
    world.diffLeaveCheck = vi.fn(() => new Promise<boolean>((resolve) => (finishSave = resolve)));
    await openPalette();
    await press('Enter');

    await change(() => (world.deleting = new Set(['beta'])));
    finishSave(true);
    await settle();

    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(commandPalette.open).toBe(true);
    expect(busyIds()).toEqual([]);
  });

  it('opens straight away from the diff view when nothing is registered to wait for', async () => {
    world.showOverProject('diff', 'alpha');
    await openPalette();

    const event = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true });
    input().dispatchEvent(event);

    // In the same tick as the key, as from any other view.
    expect(spies.selectProject).toHaveBeenCalledWith('beta');
  });
});

// The badge goes by the list this window is showing: the sidebar beside a
// project or the repos list, the grid on home. Each keeps one project its
// chip filters exclude.
describe('CommandPalette and the window filters', () => {
  it('lists projects the chip filters hide, badged, and opens them untouched', async () => {
    world.filtered = [alpha];
    await openPalette();

    expect(optionNames()).toEqual(['Alpha service', 'Beta tools', 'Gamma web']);
    expect(badges()).toEqual({ alpha: [], beta: ['Filtered out'], gamma: ['Filtered out'] });

    await press('ArrowDown');
    await press('Enter');

    expect(spies.selectProject).toHaveBeenCalledWith('beta');
    expect(spies.toggleFilter).not.toHaveBeenCalled();
    expect(spies.clearFilters).not.toHaveBeenCalled();
  });

  it("lists projects the window's search field hides, badged", async () => {
    projectsListViewState.searchQuery = 'gamma';
    await openPalette();

    expect(optionNames()).toEqual(['Alpha service', 'Beta tools', 'Gamma web']);
    expect(badges()).toEqual({ alpha: ['Filtered out'], beta: ['Filtered out'], gamma: [] });
  });

  it('marks the project on screen', async () => {
    world.showProject('beta');
    await openPalette();

    expect(badges().beta).toEqual(['Current']);
  });

  it('does not badge the selected project the sidebar keeps despite the filters', async () => {
    world.showProject('beta');
    world.filtered = [alpha];
    await openPalette();

    // The sidebar appends the selected project's row when the chips exclude
    // it, so it is on screen in the list: not "Filtered out".
    expect(badges()).toEqual({ alpha: [], gamma: ['Filtered out'], beta: ['Current'] });
  });

  it('does badge that project when the search field rules it out', async () => {
    world.showProject('beta');
    world.filtered = [alpha];
    projectsListViewState.searchQuery = 'alpha';
    await openPalette();

    // The sidebar's exception is for the chips, not for a typed query.
    expect(badges().beta).toEqual(['Current', 'Filtered out']);
  });

  it('goes by the sidebar under Settings and the diff view too', async () => {
    world.showOverProject('settings', 'beta');
    world.filtered = [alpha];
    await openPalette();

    expect(badges()).toEqual({ alpha: [], gamma: ['Filtered out'], beta: ['Current'] });
  });

  it('keeps no project beside the repos list', async () => {
    world.currentRoute = { kind: 'repos' };
    world.homeActive = false;
    world.projectLayer = { kind: 'repos' };
    world.filtered = [alpha];
    await openPalette();

    expect(badges()).toEqual({ alpha: [], beta: ['Filtered out'], gamma: ['Filtered out'] });
  });

  it('does not badge the card the grid keeps after coming back from it', async () => {
    // Back on home from Beta, with a filter that no longer matches it (the
    // visit marked it read): the grid keeps its card for this visit.
    world.showHome('beta');
    world.filtered = [alpha];
    world.activeFilters = new Set(['unread']);
    requestProjectsListRestore('beta');
    captureGridStickyProject(world.activeFilters);
    await openPalette();

    expect(badges()).toEqual({ alpha: [], beta: [], gamma: ['Filtered out'] });

    // Until the filters next change: the grid drops the card then, and the
    // palette says so while still open.
    await change(() => (world.activeFilters = new Set(['unread'])));
    expect(badges()).toEqual({ alpha: [], beta: ['Filtered out'], gamma: ['Filtered out'] });
  });

  it('goes by the grid on home, not by the project layer retained under it', async () => {
    world.showHome('beta');
    world.filtered = [alpha];
    await openPalette();

    // The sidebar would keep Beta, but the sidebar is covered by the grid,
    // which is not showing it.
    expect(badges()).toEqual({ alpha: [], beta: ['Filtered out'], gamma: ['Filtered out'] });
  });

  it('lists a project being removed but does not open it', async () => {
    world.deleting = new Set(['alpha']);
    await openPalette();

    expect(options()[0].getAttribute('aria-disabled')).toBe('true');
    expect(badges().alpha).toEqual(['Removing…']);

    await press('Enter');

    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(commandPalette.open).toBe(true);
  });
});
