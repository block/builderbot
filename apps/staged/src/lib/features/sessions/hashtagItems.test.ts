import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch, BranchTimeline, ProjectNote } from '../../types';
import { getBranchTimeline, listProjectNotes } from '../../commands';
import { buildProjectHashtagItems, timelineToHashtagItems } from './hashtagItems';

vi.mock('../../commands', () => ({
  getBranchTimeline: vi.fn(),
  listProjectNotes: vi.fn(),
}));

function branch(overrides: Partial<Branch> = {}): Branch {
  return {
    id: 'branch-1',
    projectId: 'project-1',
    projectRepoId: null,
    branchName: 'feature',
    baseBranch: 'main',
    prNumber: null,
    branchType: 'local',
    workspaceName: null,
    workstationId: null,
    workspaceStatus: null,
    setupComplete: false,
    worktreePath: null,
    createdAt: 0,
    updatedAt: 0,
    prState: null,
    prChecksStatus: null,
    prReviewDecision: null,
    prMergeable: null,
    prDraft: null,
    prUrl: null,
    prUpdatedAt: null,
    prFetchedAt: null,
    prHeadSha: null,
    ...overrides,
  };
}

function emptyTimeline(overrides: Partial<BranchTimeline> = {}): BranchTimeline {
  return {
    notes: [],
    commits: [],
    reviews: [],
    images: [],
    ...overrides,
  };
}

const projectNotes: ProjectNote[] = [];

beforeEach(() => {
  vi.mocked(getBranchTimeline).mockReset();
  vi.mocked(listProjectNotes).mockReset();
  vi.mocked(listProjectNotes).mockResolvedValue(projectNotes);
});

describe('timelineToHashtagItems', () => {
  it('uses only the commit subject for commit hashtag titles', () => {
    const timeline: BranchTimeline = emptyTimeline({
      commits: [
        {
          id: 'commit-id',
          sha: 'abcdef1234567890',
          shortSha: 'abcdef1',
          subject: 'Add branch picker filtering',
          author: 'Test User',
          timestamp: 1,
          order: 0,
          sessionId: null,
          sessionStatus: null,
          completionReason: null,
        },
      ],
    });

    expect(timelineToHashtagItems(timeline)).toContainEqual(
      expect.objectContaining({
        type: 'commit',
        id: 'abcdef1234567890',
        title: 'Add branch picker filtering',
      })
    );
  });
});

describe('buildProjectHashtagItems', () => {
  it('does not load timelines for branches that are not timeline-ready', async () => {
    vi.mocked(getBranchTimeline).mockResolvedValue(emptyTimeline());

    await buildProjectHashtagItems('project-1', [
      branch({ id: 'local-unready', branchType: 'local', worktreePath: null }),
      branch({ id: 'local-ready', branchType: 'local', worktreePath: '/tmp/repo' }),
      branch({ id: 'remote-starting', branchType: 'remote', workspaceStatus: 'starting' }),
      branch({ id: 'remote-ready', branchType: 'remote', workspaceStatus: 'running' }),
    ]);

    expect(getBranchTimeline).toHaveBeenCalledTimes(2);
    expect(getBranchTimeline).toHaveBeenNthCalledWith(1, 'local-ready');
    expect(getBranchTimeline).toHaveBeenNthCalledWith(2, 'remote-ready');
  });
});
