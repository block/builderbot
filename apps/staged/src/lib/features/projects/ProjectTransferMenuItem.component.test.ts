import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectLocation } from '../../types';

const { sq } = vi.hoisted(() => ({ sq: { loaded: true, available: true } }));

// The item renders outside a real context menu here, so it passes through.
vi.mock('$lib/components/ui/context-menu', async () => {
  const { default: Passthrough } = await import('../sessions/testStubs/PassthroughStub.svelte');
  return { Item: Passthrough };
});
vi.mock('../settings/sq.svelte', () => ({ bloxAvailable: () => sq.loaded && sq.available }));
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: {
    reposByProject: new Map(),
    isProjectTransferring: () => false,
  },
}));
vi.mock('./projectActions.svelte', () => ({
  projectActions: { requestTransferProject: vi.fn() },
}));

import ProjectTransferMenuItem from './ProjectTransferMenuItem.svelte';

let component: ReturnType<typeof mount> | null = null;

function mountItem(location: ProjectLocation): HTMLElement {
  const project = {
    id: `project-${location}`,
    name: 'Project',
    githubRepo: 'acme/widgets',
    location,
    subpath: null,
    createdAt: 1,
    updatedAt: 1,
  } as Project;
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ProjectTransferMenuItem, { target, props: { project } });
  flushSync();
  return target;
}

describe('ProjectTransferMenuItem', () => {
  beforeEach(() => {
    sq.loaded = true;
    sq.available = true;
  });

  afterEach(() => {
    if (component) unmount(component);
    component = null;
    document.body.innerHTML = '';
  });

  it('offers the other location when the sq CLI is available', () => {
    expect(mountItem('local').textContent).toContain('Transfer to Blox');
    unmount(component!);
    component = null;
    expect(mountItem('remote').textContent).toContain('Transfer to local');
  });

  it.each<ProjectLocation>(['local', 'remote'])(
    'is hidden for a %s project when the sq CLI is unavailable',
    (location) => {
      sq.available = false;
      expect(mountItem(location).textContent).not.toContain('Transfer to');
    }
  );

  it('stays hidden until the sq check has landed', () => {
    sq.loaded = false;
    sq.available = false;
    expect(mountItem('local').textContent).not.toContain('Transfer to');
  });
});
