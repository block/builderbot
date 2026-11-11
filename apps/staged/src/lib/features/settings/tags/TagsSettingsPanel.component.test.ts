// Mounts the real panel over the real tagsStore and tagActions, with the
// backend commands mocked: the assertions are on what the user sees and on
// which commands a gesture ends in. Menus and dialogs are the real bits-ui
// ones, opened and clicked the way a user would.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DeleteTagGroupMode, NewTag, TagState } from '../../../types';
import { withTagMoved, withoutGroup, withoutTag } from '../../tags/tagCatalogPatches';
import { scopeIds, tagState } from '../../tags/testStubs/tagFixtures';

const { backend, toastError } = vi.hoisted(() => ({
  backend: { state: null as unknown as TagState },
  toastError: vi.fn(),
}));

const commandsMock = vi.hoisted(() => ({
  getTagState: vi.fn(async () => ({
    data: structuredClone(backend.state),
    revalidating: null,
  })),
  createTagGroup: vi.fn(async (name: string, _tags: NewTag[]) => {
    const group = {
      id: `group-${name}`,
      name,
      position: backend.state.groups.length,
      isIconGroup: false,
      createdAt: 1,
      updatedAt: 1,
    };
    backend.state = { ...backend.state, groups: [...backend.state.groups, group] };
    return group;
  }),
  deleteTagGroup: vi.fn(async (_groupId: string, _mode: DeleteTagGroupMode) => {}),
  moveTag: vi.fn(async (_tagId: string, _groupId: string | null, _index: number) => {}),
  createTag: vi.fn(),
  updateTag: vi.fn(async () => {}),
  deleteTag: vi.fn(async (_tagId: string) => {}),
  renameTagGroup: vi.fn(async () => {}),
  setIconTagGroup: vi.fn(async () => {}),
  reorderTagGroups: vi.fn(async () => {}),
}));

vi.mock('../../../commands', () => commandsMock);
vi.mock('../../../api/commands', () => commandsMock);
vi.mock('../../../transport', () => ({ listenToEvent: () => () => {} }));
vi.mock('../../../stores/projectsData.svelte', () => ({
  projectsDataStore: {
    liveProjects: [{ id: 'a' }, { id: 'b' }, { id: 'c' }],
    isProjectDeleting: () => false,
  },
}));
vi.mock('svelte-sonner', () => ({ toast: { error: toastError } }));

import { tagsStore } from '../../../stores/tags.svelte';
import TagsSettingsPanel from './TagsSettingsPanel.svelte';

let component: ReturnType<typeof mount> | null = null;

/** Let bits-ui's floating positioning, focus moves and re-renders land. */
async function settle() {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
  }
}

async function mountPanel(state: TagState): Promise<HTMLElement> {
  backend.state = state;
  await tagsStore.load({ force: true });
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(TagsSettingsPanel, { target });
  await settle();
  return target;
}

function nameInputs(root: ParentNode, label: string): string[] {
  return [...root.querySelectorAll<HTMLInputElement>(`input[aria-label="${label}"]`)].map(
    (input) => input.value
  );
}

function row(tagId: string): HTMLElement {
  return document.querySelector<HTMLElement>(`[data-tag-row="${tagId}"]`)!;
}

/** Open a menu from its trigger and return its content. */
async function openMenu(trigger: HTMLElement): Promise<HTMLElement> {
  trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  await settle();
  const menus = document.querySelectorAll<HTMLElement>('[data-slot="dropdown-menu-content"]');
  return menus[menus.length - 1];
}

function menuItem(content: ParentNode, text: string): HTMLElement {
  const item = [...content.querySelectorAll<HTMLElement>('[role^="menuitem"]')].find((el) =>
    el.textContent?.includes(text)
  );
  if (!item) throw new Error(`No menu item "${text}"`);
  return item;
}

function dialog(): HTMLElement {
  return document.querySelector<HTMLElement>('[role="alertdialog"]')!;
}

function button(root: ParentNode, text: string): HTMLButtonElement {
  const found = [...root.querySelectorAll<HTMLButtonElement>('button')].find(
    (el) => el.textContent?.trim() === text
  );
  if (!found) throw new Error(`No button "${text}"`);
  return found;
}

function drag(eventName: string, el: HTMLElement) {
  el.dispatchEvent(new MouseEvent(eventName, { bubbles: true, cancelable: true, clientY: 10 }));
}

beforeAll(() => {
  // jsdom lacks the layout APIs the menus' positioning and viewport use.
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

afterAll(() => {
  delete (window as Partial<Window>).matchMedia;
  delete (globalThis as Partial<typeof globalThis>).ResizeObserver;
});

beforeEach(() => {
  vi.clearAllMocks();
  // The writes change what the next get_tag_state returns, as the backend's
  // would; the catalog patches mirror the backend's rules.
  commandsMock.moveTag.mockImplementation(async (tagId, groupId, index) => {
    backend.state = withTagMoved(backend.state, { tagId, groupId, index });
  });
  commandsMock.deleteTagGroup.mockImplementation(async (groupId, mode) => {
    backend.state = withoutGroup(backend.state, groupId, mode);
  });
  commandsMock.deleteTag.mockImplementation(async (tagId: string) => {
    backend.state = withoutTag(backend.state, tagId);
  });
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('TagsSettingsPanel', () => {
  it('renders each group with its tags and usage, then the ungrouped tags', async () => {
    const root = await mountPanel(
      tagState([
        { projectId: 'a', tagId: 'doing' },
        { projectId: 'b', tagId: 'doing' },
        { projectId: 'a', tagId: 'q4' },
      ])
    );

    expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority']);
    expect(nameInputs(root, 'Tag name')).toEqual([
      'Idea',
      'Doing',
      'Done',
      'P0',
      'P1',
      'q4',
      'infra',
    ]);
    expect(row('doing').textContent).toContain('Used by 2');
    expect(row('q4').textContent).toContain('Used by 1');
    expect(row('idea').textContent).toContain('Unused');

    const sections = root.querySelectorAll('section');
    expect(sections[0].textContent).toContain('Project icon');
    expect(sections[1].textContent).not.toContain('Project icon');
    expect(sections[2].textContent).toContain('Ungrouped tags');
  });

  it('creates a group from a template, with its tags and a free name', async () => {
    const root = await mountPanel(tagState());

    const content = await openMenu(root.querySelector<HTMLElement>('.new-tag-group-trigger')!);
    menuItem(content, 'Priority').click();
    await settle();

    // Priority is taken, so the new group is numbered.
    expect(commandsMock.createTagGroup).toHaveBeenCalledWith('Priority 2', [
      { name: 'P0', icon: 'chevrons-up', color: 'red' },
      { name: 'P1', icon: 'chevron-up', color: 'amber' },
      { name: 'P2', icon: 'minus', color: 'blue' },
      { name: 'P3', icon: 'chevron-down', color: 'gray' },
    ]);
    expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority', 'Priority 2']);
  });

  describe('deleting a group', () => {
    async function openDeleteDialog(groupName: string) {
      const trigger = document.querySelector<HTMLElement>(
        `[aria-label="Actions for ${groupName}"]`
      )!;
      menuItem(await openMenu(trigger), 'Delete group…').click();
      await settle();
      return dialog();
    }

    it('ungroups its tags by default', async () => {
      await mountPanel(tagState([{ projectId: 'a', tagId: 'p0' }]));
      const body = await openDeleteDialog('Priority');

      expect(body.textContent).toContain('Delete “Priority”?');
      expect(body.textContent).toContain('Keep its 2 tags as ungrouped tags');
      expect(body.textContent).toContain('1 project loses theirs');
      expect(body.querySelector<HTMLInputElement>('input[value="ungroup"]')!.checked).toBe(true);

      button(body, 'Delete group').click();
      await settle();
      expect(commandsMock.deleteTagGroup).toHaveBeenCalledWith('priority', 'ungroup');
    });

    it('deletes the tags too when asked', async () => {
      await mountPanel(tagState());
      const body = await openDeleteDialog('Status');

      expect(body.textContent).toContain('Projects will show their PR status icon instead.');
      const deleteTags = body.querySelector<HTMLInputElement>('input[value="deleteTags"]')!;
      deleteTags.click();
      flushSync();
      button(body, 'Delete group').click();
      await settle();

      expect(commandsMock.deleteTagGroup).toHaveBeenCalledWith('status', 'deleteTags');
      // The group is gone, its tags with it.
      expect(nameInputs(document, 'Group name')).toEqual(['Priority']);
      expect(document.querySelector('[data-tag-row="idea"]')).toBeNull();
    });
  });

  describe('moving a tag', () => {
    it('reorders within a group by drag, showing the new order at once', async () => {
      let finish!: () => void;
      commandsMock.moveTag.mockImplementationOnce(
        (tagId, groupId, index) =>
          new Promise<void>((resolve) => {
            finish = () => {
              backend.state = withTagMoved(backend.state, { tagId, groupId, index });
              resolve();
            };
          })
      );
      const root = await mountPanel(tagState());

      // Drop Idea below Done (the lower half of its row).
      drag('dragstart', row('idea').querySelector('.drag-handle')!);
      drag('dragover', row('done'));
      drag('drop', row('done'));
      await settle();

      expect(commandsMock.moveTag).toHaveBeenCalledWith('idea', 'status', 2);
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Doing', 'Done', 'Idea']);

      finish();
      await settle();
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Doing', 'Done', 'Idea']);
    });

    it('asks first when projects already have a tag in the target group', async () => {
      await mountPanel(
        tagState([
          { projectId: 'a', tagId: 'q4' },
          { projectId: 'a', tagId: 'doing' },
          { projectId: 'b', tagId: 'q4' },
        ])
      );

      drag('dragstart', row('q4').querySelector('.drag-handle')!);
      drag('dragover', row('done'));
      drag('drop', row('done'));
      await settle();

      expect(commandsMock.moveTag).not.toHaveBeenCalled();
      const body = dialog();
      expect(body.textContent).toContain('Move ‘q4’ to Status?');
      expect(body.textContent).toContain('1 project already has a Status.');

      button(body, 'Move').click();
      await settle();
      expect(commandsMock.moveTag).toHaveBeenCalledWith('q4', 'status', 3);
      expect(scopeIds(tagsStore.snapshot, 'status')).toEqual(['idea', 'doing', 'done', 'q4']);
    });

    it('moves without asking when no project conflicts, from the row menu', async () => {
      await mountPanel(tagState([{ projectId: 'a', tagId: 'q4' }]));

      const content = await openMenu(
        row('q4').querySelector<HTMLElement>('.tag-row-menu-trigger')!
      );
      menuItem(content, 'Move to group').click();
      await settle();
      menuItem(document, 'Priority').click();
      await settle();

      expect(commandsMock.moveTag).toHaveBeenCalledWith('q4', 'priority', 2);
      expect(dialog()).toBeNull();
    });

    it('refuses a name the target group already has', async () => {
      const state = tagState();
      state.tags.find((t) => t.id === 'q4')!.name = 'done';
      await mountPanel(state);

      drag('dragstart', row('q4').querySelector('.drag-handle')!);
      drag('dragover', row('done'));
      drag('drop', row('done'));
      await settle();

      expect(commandsMock.moveTag).not.toHaveBeenCalled();
      expect(toastError).toHaveBeenCalledWith('Status already has a tag named “Done”', {
        description: 'Rename one of them first.',
      });
    });
  });

  it('confirms deleting a tag with how many projects use it', async () => {
    await mountPanel(
      tagState([
        { projectId: 'a', tagId: 'infra' },
        { projectId: 'b', tagId: 'infra' },
      ])
    );

    row('infra').querySelector<HTMLButtonElement>('[aria-label="Delete infra"]')!.click();
    await settle();
    const body = dialog();
    expect(body.textContent).toContain('Delete “infra”? Used by 2 projects, which lose it.');

    button(body, 'Delete').click();
    await settle();
    expect(commandsMock.deleteTag).toHaveBeenCalledWith('infra');
  });
});
