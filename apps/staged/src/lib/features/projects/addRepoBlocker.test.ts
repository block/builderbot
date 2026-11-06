import { describe, expect, it } from 'vitest';
import type { Branch, Project } from '../../types';
import { addRepoBlocker } from './addRepoBlocker';

const remote = { id: 'project-1', location: 'remote' } as Project;
const local = { id: 'project-1', location: 'local' } as Project;

function remoteBranch(workspaceStatus: Branch['workspaceStatus']): Branch {
  return {
    id: `branch-${workspaceStatus}`,
    branchType: 'remote',
    workspaceName: 'stg-project',
    workspaceStatus,
  } as Branch;
}

describe('addRepoBlocker', () => {
  it('never blocks a local project', () => {
    expect(addRepoBlocker(local, [])).toBeNull();
  });

  it('lets a remote project with no workspace yet add the repo that provisions it', () => {
    expect(addRepoBlocker(remote, [])).toBeNull();
  });

  it('ignores a remote branch with a blank workspace name, like the backend', () => {
    const blank = { ...remoteBranch(null), workspaceName: '  ' };
    expect(addRepoBlocker(remote, [blank])).toBeNull();
  });

  it('lets a repo join a running workspace', () => {
    expect(addRepoBlocker(remote, [remoteBranch('running')])).toBeNull();
  });

  it('holds a second repo while the first one provisions the workspace', () => {
    expect(addRepoBlocker(remote, [remoteBranch('starting')])).toBe(
      'Workspace is provisioning. Wait until it is running, then add another repo.'
    );
  });

  it('holds a repo until a stopped, suspended or failed workspace is running', () => {
    for (const status of ['stopped', 'suspended', 'error'] as const) {
      expect(addRepoBlocker(remote, [remoteBranch(status)])).toBe(
        'Workspace must be running before adding another repo.'
      );
    }
  });
});
