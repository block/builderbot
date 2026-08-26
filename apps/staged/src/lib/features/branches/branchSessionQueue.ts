import type { BranchSessionType } from '../../types';

type TimelineSessionItem = {
  sessionStatus: string | null;
};

export interface BranchSessionQueueTimeline {
  commits: TimelineSessionItem[];
  notes: TimelineSessionItem[];
  reviews: TimelineSessionItem[];
}

export interface BranchSessionQueueOptions {
  mode: BranchSessionType;
  timeline: BranchSessionQueueTimeline | null | undefined;
  hasPendingSessionStart?: boolean;
  hasPendingQueuedSession?: boolean;
}

function isQueued(status: string | null | undefined): boolean {
  return status === 'queued';
}

function isRunning(status: string | null | undefined): boolean {
  return status === 'running';
}

export function hasQueuedBranchSession(timeline: BranchSessionQueueTimeline): boolean {
  return (
    timeline.commits.some((commit) => isQueued(commit.sessionStatus)) ||
    timeline.notes.some((note) => isQueued(note.sessionStatus)) ||
    timeline.reviews.some((review) => isQueued(review.sessionStatus))
  );
}

function runningBranchSessionTypes(timeline: BranchSessionQueueTimeline): Set<BranchSessionType> {
  const running = new Set<BranchSessionType>();

  if (timeline.commits.some((commit) => isRunning(commit.sessionStatus))) {
    running.add('commit');
  }
  if (timeline.notes.some((note) => isRunning(note.sessionStatus))) {
    running.add('note');
  }
  if (timeline.reviews.some((review) => isRunning(review.sessionStatus))) {
    running.add('review');
  }

  return running;
}

export function canStartBranchSession({
  mode,
  timeline,
  hasPendingSessionStart = false,
  hasPendingQueuedSession = false,
}: BranchSessionQueueOptions): boolean {
  if (!timeline) return false;
  if (hasPendingSessionStart || hasPendingQueuedSession) return false;
  if (hasQueuedBranchSession(timeline)) return false;

  const running = runningBranchSessionTypes(timeline);
  if (running.has('commit')) return false;
  if (mode === 'note') return true;
  if (mode === 'commit') return running.size === 0;

  return !running.has(mode);
}

export function shouldQueueBranchSession(options: BranchSessionQueueOptions): boolean {
  return !canStartBranchSession(options);
}
