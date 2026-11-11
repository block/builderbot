import { mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectLocation } from '../../types';

// The row's status icon and store-derived status are stubbed: these tests
// cover where the remote marker lands, not what the status icon shows.
vi.mock('./ProjectStatusIcon.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('./projectStatus', () => ({
  getProjectStatus: () => ({ kind: 'idle', runningCount: 0, runActionPhase: null }),
}));
vi.mock('./workspaceLifecycle.svelte', () => ({
  workspaceLifecycle: { getWorktreeErrors: () => new Map() },
}));
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: {
    branchesByProject: new Map(),
    deletingProjectNames: new Map(),
    reposByProject: new Map(),
    repoCountsByProject: new Map(),
    isProjectHydrated: () => true,
  },
}));
vi.mock('../../stores/projectState.svelte', () => ({
  projectStateStore: { getRunningSessionTypes: () => [] },
}));
vi.mock('../../stores/repoBadges.svelte', () => ({
  repoBadgeStore: { lookup: () => null },
}));

import ProjectRowContent from './ProjectRowContent.svelte';

const MARKER = '[aria-label="Remote project"]';

let component: ReturnType<typeof mount> | null = null;

function mountRow(location: ProjectLocation) {
  const project = {
    id: `project-${location}`,
    name: 'Project',
    githubRepo: null,
    location,
    subpath: null,
    createdAt: 1,
    updatedAt: 1,
  } as Project;
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ProjectRowContent, { target, props: { project } });
  return target;
}

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('ProjectRowContent remote marker', () => {
  it('renders one cloud inside the row for a remote project', () => {
    const target = mountRow('remote');

    expect(target.querySelectorAll(MARKER)).toHaveLength(1);
    expect(target.querySelector(`.row-main ${MARKER}`)).not.toBeNull();
  });

  it('renders no cloud for a local project', () => {
    const target = mountRow('local');

    expect(target.querySelector('.row-main')).not.toBeNull();
    expect(target.querySelector(MARKER)).toBeNull();
  });

  it('places the cloud after the name and before the meta line', () => {
    const target = mountRow('remote');
    const name = target.querySelector('.project-name')!;
    const marker = target.querySelector(MARKER)!;
    const meta = target.querySelector('.row-meta')!;

    expect(name.compareDocumentPosition(marker) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(marker.compareDocumentPosition(meta) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});
