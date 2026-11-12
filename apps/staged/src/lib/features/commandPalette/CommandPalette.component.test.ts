import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch, Project, ProjectRepo } from '../../types';

// The palette under the real dialog primitive (focus scope, Escape layer,
// dismissible layer), with the stores it reads replaced by plain data the
// tests own. What is covered here is the wiring: keyboard and pointer
// selection, opening a result, open/close and focus. The matching itself is
// covered in paletteSearch.test.ts.

const world = vi.hoisted(() => ({
  projects: [] as Project[],
  repos: new Map<string, ProjectRepo[]>(),
  branches: new Map<string, Branch[]>(),
  deleting: new Set<string>(),
  loaded: true,
  // What the window's chip filters currently show, or null for no filters.
  filtered: null as Project[] | null,
  sidebarQuery: '',
  selectedProjectId: null as string | null,
  currentRoute: { kind: 'projects' } as { kind: string; projectId?: string },
}));

const spies = vi.hoisted(() => ({
  selectProject: vi.fn(),
  ensureProjectsHydrated: vi.fn(async () => {}),
  ensureIndexLoaded: vi.fn(),
  toggleFilter: vi.fn(),
  clearFilters: vi.fn(),
  revealWithin: vi.fn(),
}));

vi.mock('../../stores/projectsData.svelte', () => ({
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
}));
vi.mock('../layout/navigation.svelte', () => ({
  navigation: {
    get selectedProjectId() {
      return world.selectedProjectId;
    },
    get currentRoute() {
      return world.currentRoute;
    },
  },
  selectProject: spies.selectProject,
}));
vi.mock('../projects/projectFilters.svelte', () => ({
  projectFiltersStore: {
    get hasActiveFilters() {
      return world.filtered !== null;
    },
    get filteredProjects() {
      return world.filtered ?? world.projects;
    },
    toggleFilter: spies.toggleFilter,
    clearFilters: spies.clearFilters,
  },
}));
vi.mock('../projects/projectsListViewState.svelte', () => ({
  projectsListViewState: {
    get searchQuery() {
      return world.sidebarQuery;
    },
  },
}));
vi.mock('../projects/projectSearchIndex.svelte', () => ({
  projectSearchIndex: { index: null, ensureLoaded: spies.ensureIndexLoaded },
}));
vi.mock('../projects/projectSearchContext', async () => {
  const { buildSearchSnapshot } = await import('../projects/projectSearch');
  return {
    projectSearchSnapshots: () => () =>
      buildSearchSnapshot(world.projects, {
        branchesByProject: world.branches,
        reposByProject: world.repos,
      }),
  };
});
// The status icon reads preferences and PR state; not what these tests cover.
vi.mock(
  '../projects/ProjectStatusIcon.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('../../shared/revealWithin', () => ({ revealWithin: spies.revealWithin }));

import CommandPalette from './CommandPalette.svelte';
import {
  closeCommandPalette,
  commandPalette,
  openCommandPalette,
  otherModalOpen,
  toggleCommandPalette,
} from './paletteState.svelte';

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

const dialog = () => document.querySelector<HTMLElement>('[data-command-palette]');
/** The dialog leaves the DOM a frame or two after it closes. */
const dialogGone = () => vi.waitFor(() => expect(dialog()).toBeNull());
const input = () => document.querySelector<HTMLInputElement>('[data-command-palette] input')!;
const options = () => [
  ...document.querySelectorAll<HTMLElement>('[data-command-palette] [role="option"]'),
];
const optionNames = () => options().map((el) => el.querySelector('.row-name')?.textContent);
const selectedIds = () =>
  options()
    .filter((el) => el.getAttribute('aria-selected') === 'true')
    .map((el) => el.dataset.projectId);

async function press(key: string, init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init });
  input().dispatchEvent(event);
  await settle();
  return event;
}

async function type(text: string) {
  const el = input();
  el.value = text;
  el.dispatchEvent(new Event('input', { bubbles: true }));
  await settle();
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
  world.projects = [gamma, alpha, beta];
  world.repos = new Map();
  world.branches = new Map();
  world.deleting = new Set();
  world.loaded = true;
  world.filtered = null;
  world.sidebarQuery = '';
  world.selectedProjectId = null;
  world.currentRoute = { kind: 'projects' };
  for (const spy of Object.values(spies)) spy.mockClear();

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
    const before = document.createElement('button');
    document.body.appendChild(before);
    before.focus();
    await openPalette();
    expect(document.activeElement).toBe(input());

    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await settle();

    expect(commandPalette.open).toBe(false);
    await dialogGone();
    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(before);
  });

  it('closes when the shortcut toggles it again', async () => {
    await openPalette();

    expect(toggleCommandPalette()).toBe(true);
    await settle();

    expect(commandPalette.open).toBe(false);
    await dialogGone();
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
    world.selectedProjectId = 'alpha';
    world.currentRoute = { kind: 'project', projectId: 'alpha' };
    await openPalette();
    // The project on screen is listed last.
    expect(optionNames()).toEqual(['Beta tools', 'Gamma web', 'Alpha service']);
    await press('ArrowUp');
    expect(selectedIds()).toEqual(['alpha']);

    await press('Enter');

    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(commandPalette.open).toBe(false);
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
    expect(document.querySelector('[data-command-palette] [role="status"]')?.textContent).toContain(
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
});

describe('CommandPalette pointer', () => {
  it('moves the highlight to the row under the pointer', async () => {
    await openPalette();

    options()[2].dispatchEvent(new MouseEvent('pointermove', { bubbles: true }));
    await settle();

    expect(selectedIds()).toEqual(['gamma']);
    // Hovering doesn't scroll the list out from under the pointer.
    expect(spies.revealWithin).not.toHaveBeenCalled();
  });

  it('opens the clicked project and closes', async () => {
    await openPalette();

    options()[1].click();
    await settle();

    expect(spies.selectProject).toHaveBeenCalledWith('beta');
    expect(commandPalette.open).toBe(false);
  });

  it('keeps focus in the input when a row is pressed', async () => {
    await openPalette();

    const down = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    options()[1].dispatchEvent(down);

    expect(down.defaultPrevented).toBe(true);
  });
});

describe('CommandPalette and the window filters', () => {
  it('lists projects the chip filters hide, badged, and opens them untouched', async () => {
    world.filtered = [alpha];
    await openPalette();

    expect(optionNames()).toEqual(['Alpha service', 'Beta tools', 'Gamma web']);
    const badges = options().map((el) => el.querySelector('.row-badges')?.textContent?.trim());
    expect(badges).toEqual([undefined, 'Filtered out', 'Filtered out']);

    await press('ArrowDown');
    await press('Enter');

    expect(spies.selectProject).toHaveBeenCalledWith('beta');
    expect(spies.toggleFilter).not.toHaveBeenCalled();
    expect(spies.clearFilters).not.toHaveBeenCalled();
  });

  it("lists projects the sidebar's search hides, badged", async () => {
    world.sidebarQuery = 'gamma';
    await openPalette();

    const badges = options().map((el) => el.querySelector('.row-badges')?.textContent?.trim());
    expect(optionNames()).toEqual(['Alpha service', 'Beta tools', 'Gamma web']);
    expect(badges).toEqual(['Filtered out', 'Filtered out', undefined]);
  });

  it('marks the project on screen', async () => {
    world.selectedProjectId = 'beta';
    world.currentRoute = { kind: 'project', projectId: 'beta' };
    await openPalette();

    const current = options().find((el) => el.dataset.projectId === 'beta')!;
    expect(current.querySelector('.row-badges')?.textContent?.trim()).toBe('Current');
  });

  it('lists a project being removed but does not open it', async () => {
    world.deleting = new Set(['alpha']);
    await openPalette();

    expect(options()[0].getAttribute('aria-disabled')).toBe('true');
    expect(options()[0].querySelector('.row-badges')?.textContent?.trim()).toBe('Removing…');

    await press('Enter');

    expect(spies.selectProject).not.toHaveBeenCalled();
    expect(commandPalette.open).toBe(true);
  });
});

describe('the shortcut and other modals', () => {
  function addModal(attributes: Record<string, string>) {
    const modal = document.createElement('div');
    for (const [name, value] of Object.entries(attributes)) modal.setAttribute(name, value);
    document.body.appendChild(modal);
    return modal;
  }

  it('does not open while another dialog is open', async () => {
    addModal({ role: 'dialog', 'data-state': 'open' });

    expect(otherModalOpen()).toBe(true);
    expect(toggleCommandPalette()).toBe(false);
    await settle();

    expect(commandPalette.open).toBe(false);
    await dialogGone();
  });

  it('does not open over a pending confirmation', () => {
    addModal({ role: 'alertdialog', 'data-state': 'open' });

    expect(toggleCommandPalette()).toBe(false);
    expect(commandPalette.open).toBe(false);
  });

  it('ignores a dialog that is only animating out', async () => {
    addModal({ role: 'dialog', 'data-state': 'closed' });

    expect(otherModalOpen()).toBe(false);
    expect(toggleCommandPalette()).toBe(true);
    await settle();

    expect(dialog()).not.toBeNull();
  });

  it('does not count itself, so the shortcut still closes it', async () => {
    await openPalette();

    expect(otherModalOpen()).toBe(false);
    expect(toggleCommandPalette()).toBe(true);
    expect(commandPalette.open).toBe(false);
  });
});
