import type { DiffDetailRoute } from '../layout/navigation.svelte';
import type { DisplayRootInput } from '../sessions/pathDisplayRoots';
import type { HashtagItem, ProjectRepo } from '../../types';

export type HashtagClickInfo = {
  type: HashtagItem['type'];
  id: string;
  ref: string;
  item?: HashtagItem;
};

export type ReferenceDiffContext = Partial<
  Pick<
    DiffDetailRoute,
    | 'projectId'
    | 'commits'
    | 'baseBranchLabel'
    | 'branchLabel'
    | 'projectName'
    | 'githubRepo'
    | 'subpath'
  >
> & {
  branchId?: string;
};

type ReferenceDialogContext = {
  hashtagItems?: HashtagItem[];
  diffContext?: ReferenceDiffContext;
};

export type ReferenceNoteEntry = ReferenceDialogContext & {
  kind: 'note';
  noteKind: 'branch' | 'project';
  id: string;
  ref: string;
  title: string;
  content: string;
  view: 'note' | 'chat';
  sessionId?: string | null;
  noteUpdatedAt?: number | null;
  branchId?: string | null;
  projectId?: string | null;
};

export type ReferenceChatEntry = ReferenceDialogContext & {
  kind: 'chat';
  ref: string;
  sessionId: string;
  branchId?: string | null;
  projectId?: string | null;
  repoDir?: DisplayRootInput;
  repoLabel?: Pick<ProjectRepo, 'githubRepo' | 'subpath' | 'headRepo'> | null;
};

export type ReferenceImageEntry = ReferenceDialogContext & {
  kind: 'image';
  ref: string;
  imageId: string;
  filename: string;
  branchId?: string | null;
  projectId?: string | null;
};

export type ReferenceDiffEntry = {
  kind: 'diff';
  ref: string;
  route: Omit<DiffDetailRoute, 'kind'>;
};

export type ReferenceHistoryEntry =
  ReferenceNoteEntry | ReferenceChatEntry | ReferenceImageEntry | ReferenceDiffEntry;

export type ReferenceNavState = {
  canGoBack: boolean;
  canGoForward: boolean;
  onBack: () => void;
  onForward: () => void;
};

export const disabledReferenceNav: ReferenceNavState = {
  canGoBack: false,
  canGoForward: false,
  onBack: () => {},
  onForward: () => {},
};

export const referenceHistory = $state({
  entries: [] as ReferenceHistoryEntry[],
  index: -1,
});

export function currentReferenceEntry(): ReferenceHistoryEntry | null {
  return referenceHistory.entries[referenceHistory.index] ?? null;
}

export function referenceCanGoBack(): boolean {
  return referenceHistory.index > 0;
}

export function referenceCanGoForward(): boolean {
  return (
    referenceHistory.index >= 0 && referenceHistory.index < referenceHistory.entries.length - 1
  );
}

export function pushReferenceEntry(entry: ReferenceHistoryEntry): void {
  const preserved = referenceHistory.entries.slice(0, referenceHistory.index + 1);
  referenceHistory.entries = [...preserved, entry];
  referenceHistory.index = referenceHistory.entries.length - 1;
}

export function replaceCurrentReferenceEntry(entry: ReferenceHistoryEntry): void {
  if (referenceHistory.index < 0) {
    pushReferenceEntry(entry);
    return;
  }

  referenceHistory.entries = referenceHistory.entries.map((existing, index) =>
    index === referenceHistory.index ? entry : existing
  );
}

export function clearReferenceHistory(): void {
  referenceHistory.entries = [];
  referenceHistory.index = -1;
}

export function moveReferenceBack(): ReferenceHistoryEntry | null {
  if (!referenceCanGoBack()) return currentReferenceEntry();
  referenceHistory.index -= 1;
  return currentReferenceEntry();
}

export function moveReferenceForward(): ReferenceHistoryEntry | null {
  if (!referenceCanGoForward()) return currentReferenceEntry();
  referenceHistory.index += 1;
  return currentReferenceEntry();
}

export function restorePreviousReferenceAfterDiffRoute(): void {
  const current = currentReferenceEntry();
  if (current?.kind !== 'diff') return;
  if (referenceHistory.index > 0) {
    referenceHistory.index -= 1;
  } else {
    clearReferenceHistory();
  }
}

export function setCurrentNoteView(view: 'note' | 'chat'): void {
  const current = currentReferenceEntry();
  if (current?.kind !== 'note') return;
  replaceCurrentReferenceEntry({ ...current, view });
}

export function resolveHashtagReference(
  click: HashtagClickInfo,
  context: ReferenceDialogContext = {}
): ReferenceHistoryEntry | null {
  const item =
    click.item ??
    context.hashtagItems?.find((candidate) => {
      return candidate.type === click.type && candidate.id === click.id;
    });

  switch (click.type) {
    case 'note':
    case 'project-note':
      if (!item?.noteContent && item?.noteContent !== '') return null;
      return {
        kind: 'note',
        noteKind: click.type === 'project-note' ? 'project' : 'branch',
        id: click.id,
        ref: click.ref,
        title: item.title,
        content: item.noteContent,
        view: 'note',
        sessionId: item.noteSessionId,
        noteUpdatedAt: item.noteUpdatedAt,
        branchId: item.branchId,
        projectId: item.projectId,
        hashtagItems: context.hashtagItems,
        diffContext: context.diffContext,
      };

    case 'image':
      if (!item?.imageFilename) return null;
      return {
        kind: 'image',
        ref: click.ref,
        imageId: click.id,
        filename: item.imageFilename,
        branchId: item.branchId,
        projectId: item.projectId,
        hashtagItems: context.hashtagItems,
        diffContext: context.diffContext,
      };

    case 'commit': {
      const route = diffRouteForHashtag(item, click, context);
      if (!route) return null;
      return { kind: 'diff', ref: click.ref, route };
    }

    case 'review': {
      const route = diffRouteForHashtag(item, click, context);
      if (!route) return null;
      return { kind: 'diff', ref: click.ref, route };
    }
  }
}

function diffRouteForHashtag(
  item: HashtagItem | undefined,
  click: HashtagClickInfo,
  context: ReferenceDialogContext
): Omit<DiffDetailRoute, 'kind'> | null {
  const branchId = item?.branchId ?? context.diffContext?.branchId;
  if (!branchId) return null;

  const projectId = item?.projectId ?? context.diffContext?.projectId;
  const shared = {
    projectId,
    commits: context.diffContext?.commits,
    baseBranchLabel: context.diffContext?.baseBranchLabel,
    branchLabel: context.diffContext?.branchLabel,
    projectName: context.diffContext?.projectName,
    githubRepo: context.diffContext?.githubRepo ?? item?.repoSlug,
    subpath: context.diffContext?.subpath ?? item?.repoSubpath,
  };

  if (click.type === 'commit') {
    return {
      ...shared,
      branchId,
      commitSha: click.id,
      scope: 'commit',
      beforeLabel: 'parent',
      afterLabel: click.id.slice(0, 7),
    };
  }

  const commitSha = item?.reviewCommitSha;
  if (!commitSha) return null;
  const scope = item?.reviewScope ?? 'commit';
  return {
    ...shared,
    branchId,
    commitSha,
    scope,
    reviewId: click.id,
    beforeLabel: scope === 'commit' ? 'parent' : context.diffContext?.baseBranchLabel,
    afterLabel: commitSha.slice(0, 7),
  };
}
