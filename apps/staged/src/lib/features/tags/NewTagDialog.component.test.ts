// The context menu's "New tag…" dialog over the real tagsStore and
// tagActions, with the dialog chrome stubbed and the commands mocked.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, Tag, TagState } from '../../types';
import { tagState } from './testStubs/tagFixtures';

const { backend } = vi.hoisted(() => ({ backend: { state: null as unknown as TagState } }));

const commandsMock = vi.hoisted(() => ({
  getTagState: vi.fn(async () => ({ data: structuredClone(backend.state), revalidating: null })),
  setProjectTagAssigned: vi.fn(
    async (_projectId: string, _tagId: string, _assigned: boolean) => {}
  ),
  createTag: vi.fn(
    async (input: { name: string; groupId?: string | null; assignProjectId?: string | null }) => {
      const created: Tag = {
        id: `new-${input.name}`,
        groupId: input.groupId ?? null,
        name: input.name,
        color: 'blue',
        icon: null,
        position: 9,
        createdAt: 1,
        updatedAt: 1,
      };
      const projectId = input.assignProjectId;
      backend.state = {
        ...backend.state,
        tags: [...backend.state.tags, created],
        assignments: projectId
          ? [...backend.state.assignments, { projectId, tagId: created.id }]
          : backend.state.assignments,
      };
      return created;
    }
  ),
}));

vi.mock('../../commands', () => commandsMock);
vi.mock('../../api/commands', () => commandsMock);
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  listenToEvent: () => () => {},
}));
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: { liveProjects: [{ id: 'p1' }], isProjectDeleting: () => false },
}));
vi.mock('$lib/components/ui/dialog', () =>
  import('../sessions/testStubs/dialogMock').then((m) => m.dialogModuleMock())
);
vi.mock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));

import { tagsStore } from '../../stores/tags.svelte';
import NewTagDialog from './NewTagDialog.svelte';
import { newTagDialog } from './newTagRequest.svelte';

const project: Project = {
  id: 'p1',
  name: 'Project',
  githubRepo: null,
  location: 'local',
  subpath: null,
  createdAt: 1,
  updatedAt: 1,
};

let component: ReturnType<typeof mount> | null = null;

async function settle() {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
  }
}

async function submit(name: string): Promise<HTMLElement> {
  backend.state = tagState();
  await tagsStore.load({ force: true });
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(NewTagDialog, { target });
  newTagDialog.request(project);
  await settle();

  const input = target.querySelector<HTMLInputElement>('#new-tag-name')!;
  input.value = name;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
  target.querySelector('form')!.dispatchEvent(new Event('submit', { cancelable: true }));
  await settle();
  return target;
}

beforeEach(() => {
  vi.clearAllMocks();
  newTagDialog.open = false;
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('NewTagDialog', () => {
  it('creates an ungrouped tag assigned to the requested project, then closes', async () => {
    await submit('  q4-launch ');

    expect(commandsMock.createTag).toHaveBeenCalledWith({
      name: 'q4-launch',
      groupId: null,
      assignProjectId: 'p1',
    });
    expect(newTagDialog.open).toBe(false);
    expect(tagsStore.tagIdsFor('p1').has('new-q4-launch')).toBe(true);
  });

  it('assigns the ungrouped tag that already has the name instead', async () => {
    await submit('INFRA');

    expect(commandsMock.createTag).not.toHaveBeenCalled();
    expect(commandsMock.setProjectTagAssigned).toHaveBeenCalledWith('p1', 'infra', true);
    expect(newTagDialog.open).toBe(false);
  });

  it('asks for a name rather than submitting an empty one', async () => {
    const root = await submit('   ');

    expect(commandsMock.createTag).not.toHaveBeenCalled();
    expect(root.querySelector('[role="alert"]')?.textContent).toBe('Enter a tag name.');
    expect(newTagDialog.open).toBe(true);
  });
});
