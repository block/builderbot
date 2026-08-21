import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch, BranchTimeline, HashtagItem, ProjectNote } from '../../types';
import { getBranchTimeline, listProjectNotes } from '../../commands';
import {
  buildProjectHashtagItems,
  createExtractedValueBuilder,
  findHashtagItemForReference,
  projectNotesToHashtagItems,
  renderHashtagTokens,
  timelineToHashtagItems,
} from './hashtagItems';

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

function projectNote(overrides: Partial<ProjectNote> = {}): ProjectNote {
  return {
    id: 'project-note-1',
    projectId: 'project-1',
    sessionId: null,
    title: 'Project note',
    content: '',
    createdAt: 0,
    updatedAt: 0,
    completedAt: 0,
    suggestedNextCommitStep: null,
    suggestedNextNoteStep: null,
    suggestedNextSteps: [],
    sessionStatus: null,
    completionReason: null,
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
          authorEmail: 'test@example.com',
          isOwnCommit: true,
          timestamp: 1,
          sortTimestamp: 1,
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

  it('sorts timeline references newest first across types', () => {
    const timeline: BranchTimeline = emptyTimeline({
      notes: [
        {
          id: 'old-note',
          title: 'Old note',
          content: '',
          sessionId: null,
          sessionStatus: null,
          completionReason: null,
          createdAt: 1000,
          updatedAt: 1000,
          completedAt: 1000,
          suggestedNextCommitStep: null,
          suggestedNextNoteStep: null,
          suggestedNextSteps: [],
        },
        {
          id: 'new-note',
          title: 'New note',
          content: '',
          sessionId: null,
          sessionStatus: null,
          completionReason: null,
          createdAt: 5000,
          updatedAt: 5000,
          completedAt: 5000,
          suggestedNextCommitStep: null,
          suggestedNextNoteStep: null,
          suggestedNextSteps: [],
        },
      ],
      commits: [
        {
          id: 'old-commit-id',
          sha: 'oldcommit',
          shortSha: 'oldcomm',
          subject: 'Old commit',
          author: 'Test User',
          authorEmail: 'test@example.com',
          isOwnCommit: true,
          timestamp: 2000,
          sortTimestamp: 2000,
          order: 0,
          sessionId: null,
          sessionStatus: null,
          completionReason: null,
        },
        {
          id: 'new-commit-id',
          sha: 'newcommit',
          shortSha: 'newcomm',
          subject: 'New commit',
          author: 'Test User',
          authorEmail: 'test@example.com',
          isOwnCommit: true,
          timestamp: 6000,
          sortTimestamp: 6000,
          order: 1,
          sessionId: null,
          sessionStatus: null,
          completionReason: null,
        },
      ],
      reviews: [
        {
          id: 'old-review',
          commitSha: 'oldcommit',
          scope: 'commit',
          sessionId: null,
          sessionStatus: null,
          sessionProvider: null,
          completionReason: null,
          title: 'Old review',
          commentCount: 0,
          isAuto: false,
          createdAt: 3000,
          updatedAt: 3000,
          completedAt: 3000,
        },
        {
          id: 'new-review',
          commitSha: 'newcommit',
          scope: 'commit',
          sessionId: null,
          sessionStatus: null,
          sessionProvider: null,
          completionReason: null,
          title: 'New review',
          commentCount: 0,
          isAuto: false,
          createdAt: 7000,
          updatedAt: 7000,
          completedAt: 7000,
        },
      ],
      images: [
        {
          id: 'old-image',
          filename: 'old.png',
          mimeType: 'image/png',
          sizeBytes: 1,
          sessionId: null,
          sessionStatus: null,
          completionReason: null,
          createdAt: 4000,
        },
        {
          id: 'new-image',
          filename: 'new.png',
          mimeType: 'image/png',
          sizeBytes: 1,
          sessionId: null,
          sessionStatus: null,
          completionReason: null,
          createdAt: 8000,
        },
      ],
    });

    expect(timelineToHashtagItems(timeline).map((item) => `${item.type}:${item.id}`)).toEqual([
      'image:new-image',
      'review:new-review',
      'commit:newcommit',
      'note:new-note',
      'image:old-image',
      'review:old-review',
      'commit:oldcommit',
      'note:old-note',
    ]);
  });
});

describe('projectNotesToHashtagItems', () => {
  it('sorts project notes newest first without a generic subtitle', () => {
    const items = projectNotesToHashtagItems([
      projectNote({ id: 'old-project-note', title: 'Old project note', completedAt: 1000 }),
      projectNote({ id: 'new-project-note', title: 'New project note', completedAt: 2000 }),
    ]);

    expect(items.map((item) => item.id)).toEqual(['new-project-note', 'old-project-note']);
    expect(items[0]).not.toHaveProperty('subtitle');
  });
});

describe('findHashtagItemForReference', () => {
  it('resolves #note project-note aliases after exact note matches', () => {
    const items: HashtagItem[] = [
      {
        type: 'project-note',
        id: 'shared-id',
        title: 'Project note',
        color: '--note-color',
        bgColor: '--note-bg',
      },
      {
        type: 'note',
        id: 'shared-id',
        title: 'Branch note',
        color: '--note-color',
        bgColor: '--note-bg',
      },
      {
        type: 'project-note',
        id: 'project-only',
        title: 'Project only',
        color: '--note-color',
        bgColor: '--note-bg',
      },
    ];

    expect(findHashtagItemForReference(items, 'note', 'shared-id')).toEqual(
      expect.objectContaining({ type: 'note', title: 'Branch note' })
    );
    expect(findHashtagItemForReference(items, 'note', 'project-only')).toEqual(
      expect.objectContaining({ type: 'project-note', title: 'Project only' })
    );
  });
});

describe('renderHashtagTokens', () => {
  it('renders project notes from #note aliases and keeps #project-note accepted', () => {
    const items = projectNotesToHashtagItems([
      projectNote({ id: 'project-note-1', title: 'Project note title' }),
    ]);

    const noteAliasHtml = renderHashtagTokens('See #note:project-note-1', items);
    expect(noteAliasHtml).toContain('Project note title');
    expect(noteAliasHtml).toContain('data-hashtag-ref="#note:project-note-1"');
    expect(noteAliasHtml).toContain('data-hashtag-type="project-note"');
    expect(noteAliasHtml).toContain('data-hashtag-id="project-note-1"');

    const projectNoteHtml = renderHashtagTokens('See #project-note:project-note-1', items);
    expect(projectNoteHtml).toContain('Project note title');
    expect(projectNoteHtml).toContain('data-hashtag-ref="#project-note:project-note-1"');
    expect(projectNoteHtml).toContain('data-hashtag-type="project-note"');
  });

  it('can render presentational badges without interaction attributes', () => {
    const html = renderHashtagTokens(
      'See #note:note-1',
      [
        {
          type: 'note',
          id: 'note-1',
          title: 'Note title',
          color: '--note-color',
          bgColor: '--note-bg',
        },
      ],
      { interactive: false }
    );

    expect(html).toContain('class="hashtag-badge stable-raster stable-raster-glyphs"');
    expect(html).toContain('Note title');
    expect(html).toContain('viewBox="0 0 24 24"');
    expect(html).toContain('style="background: var(--note-bg); color: var(--note-color);"');
    expect(html).toContain('data-hashtag-type="note"');
    expect(html).toContain('data-hashtag-id="note-1"');
    expect(html).not.toContain('role="button"');
    expect(html).not.toContain('tabindex="0"');
    expect(html).not.toContain('data-hashtag-ref');
  });
});

describe('createExtractedValueBuilder', () => {
  it('inserts a space between a token and immediately following text', () => {
    const builder = createExtractedValueBuilder();
    builder.appendToken('#note:note-1');
    builder.appendText('some words');

    expect(builder.value).toBe('#note:note-1 some words');
  });

  it('does not add a space when the following text already starts with whitespace', () => {
    const builder = createExtractedValueBuilder();
    builder.appendText('Re: ');
    builder.appendToken('#note:note-1');
    builder.appendText(' already spaced');

    expect(builder.value).toBe('Re: #note:note-1 already spaced');
  });

  it('does not add a space before a newline after a token', () => {
    const builder = createExtractedValueBuilder();
    builder.appendToken('#commit:abc123');
    builder.appendText('\nnext line');

    expect(builder.value).toBe('#commit:abc123\nnext line');
  });

  it('separates adjacent tokens even across empty text chunks', () => {
    const builder = createExtractedValueBuilder();
    builder.appendToken('#note:note-1');
    builder.appendText('');
    builder.appendToken('#commit:abc123');

    expect(builder.value).toBe('#note:note-1 #commit:abc123');
  });

  it('does not add a trailing space after a token at the end', () => {
    const builder = createExtractedValueBuilder();
    builder.appendText('see ');
    builder.appendToken('#note:note-1');

    expect(builder.value).toBe('see #note:note-1');
  });

  it('joins plain text chunks without inserting spaces', () => {
    const builder = createExtractedValueBuilder();
    builder.appendText('hel');
    builder.appendText('lo');

    expect(builder.value).toBe('hello');
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

  it('reuses provided project notes instead of fetching them again', async () => {
    vi.mocked(getBranchTimeline).mockResolvedValue(emptyTimeline());

    const items = await buildProjectHashtagItems('project-1', [], undefined, [
      projectNote({ id: 'provided-note', title: 'Provided note' }),
    ]);

    expect(listProjectNotes).not.toHaveBeenCalled();
    expect(items).toContainEqual(
      expect.objectContaining({
        type: 'project-note',
        id: 'provided-note',
        title: 'Provided note',
      })
    );
  });
});
