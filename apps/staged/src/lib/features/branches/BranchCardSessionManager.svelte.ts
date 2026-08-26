/**
 * BranchCardSessionManager — reactive session creation logic for BranchCard
 *
 * Manages new session modal state and branch-card session start orchestration.
 *
 * Instantiated with a reactive branch reference. Exposes state as reactive
 * properties and methods. Shared branch-scoped pending session state lives in
 * branchSessionLaunch.svelte.ts so diff-launched sessions and branch-card
 * launches render through the same timeline rows.
 */

import type {
  AcpConfigSelection,
  Branch,
  BranchTimeline as BranchTimelineData,
  BranchSessionType,
} from '../../types';
import { buildReferringPrompt } from '../../shared/buildReferringPrompt';
import { shouldQueueBranchSession } from './branchSessionQueue';
import {
  getPendingSessionItems,
  hasPendingQueuedSession,
  hasPendingSessionStart,
  startOrQueueBranchSessionWithPending,
} from './branchSessionLaunch.svelte';

interface BranchSessionLaunchOptions {
  provider?: string | null;
  acpConfigSelection?: AcpConfigSelection | null;
}

export default class BranchCardSessionManager {
  // Private callback refs — declared first so $derived fields can reference them
  private getBranch: () => Branch = undefined!;
  private getIsRemote: () => boolean = undefined!;
  private loadTimeline: (opts?: { timelineKey?: string | null; force?: boolean }) => void =
    undefined!;
  private getTimeline: () => BranchTimelineData | null = () => null;

  // New session modal state
  showNewSession = $state(false);
  newSessionMode = $state<BranchSessionType>('commit');
  draftPrompt = $state('');
  draftImageIds = $state<string[]>([]);
  isSessionStartPending = $derived.by(() => {
    const branch = this.getBranch?.();
    return branch ? hasPendingSessionStart(branch.id) : false;
  });
  private hasPendingQueuedSession = $derived.by(() => {
    const branch = this.getBranch?.();
    return branch ? hasPendingQueuedSession(branch.id) : false;
  });

  // Session modal (opened after starting a branch session, or from timeline)
  openSessionId = $state<string | null>(null);

  /** True when a new session will be queued rather than started immediately. */
  willQueue = $derived.by(() => this.willQueueForMode(this.newSessionMode));

  /** True when new session actions (new commit, note, review) should be disabled. */
  isNewSessionDisabled = $derived(this.showNewSession || this.isSessionStartPending);

  /** True when a commit session is pending, queued, or actively running. */
  hasCommitSessionInProgress = $derived.by(() => {
    const branch = this.getBranch?.();
    if (
      branch &&
      getPendingSessionItems(branch.id).some(
        (item) => item.type === 'pending-commit' || item.type === 'queued-commit'
      )
    ) {
      return true;
    }
    const tl = this.getTimeline();
    return (
      !!tl && tl.commits.some((c) => c.sessionStatus === 'running' || c.sessionStatus === 'queued')
    );
  });

  constructor(opts: {
    getBranch: () => Branch;
    getIsRemote: () => boolean;
    loadTimeline: (opts?: { timelineKey?: string | null; force?: boolean }) => void;
    getTimeline: () => BranchTimelineData | null;
  }) {
    this.getBranch = opts.getBranch;
    this.getIsRemote = opts.getIsRemote;
    this.loadTimeline = opts.loadTimeline;
    this.getTimeline = opts.getTimeline;
  }

  willQueueForMode(mode: BranchSessionType): boolean {
    return shouldQueueBranchSession({
      mode,
      timeline: this.getTimeline(),
      hasPendingSessionStart: this.isSessionStartPending,
      hasPendingQueuedSession: this.hasPendingQueuedSession,
    });
  }

  async startOrQueueSession(
    mode: BranchSessionType,
    prompt: string,
    imageIds: string[] = [],
    launchOptions: BranchSessionLaunchOptions = {}
  ) {
    const branch = this.getBranch();
    const isRemote = this.getIsRemote();

    await startOrQueueBranchSessionWithPending({
      branchId: branch.id,
      isRemote,
      mode,
      prompt,
      imageIds,
      provider: launchOptions.provider,
      acpConfigSelection: launchOptions.acpConfigSelection,
      getTimeline: () => this.getTimeline(),
      onTimelineRefresh: () => this.loadTimeline(),
    });
  }

  openNewSession(mode: BranchSessionType, e?: MouseEvent) {
    if (mode === 'review' && e?.altKey) {
      void this.startReviewSessionWithoutDialog();
      return;
    }
    this.newSessionMode = mode;
    this.showNewSession = true;
  }

  openNewSessionReferring(hashtagRef: string) {
    const hadContent = this.showNewSession && this.draftPrompt.trim();
    this.draftPrompt = buildReferringPrompt(this.draftPrompt, hashtagRef);
    if (!hadContent) {
      this.newSessionMode = 'commit';
      this.showNewSession = true;
    }
  }

  async startReviewSessionWithoutDialog() {
    this.newSessionMode = 'review';
    this.showNewSession = false;
    this.draftPrompt = '';
    this.draftImageIds = [];

    const reviewPrompt = 'Review the code changes on this branch.';
    await this.startOrQueueSession('review', reviewPrompt);
  }

  handleNewSessionClose(draft: { prompt: string; mode: BranchSessionType; imageIds: string[] }) {
    this.draftPrompt = draft.prompt;
    this.newSessionMode = draft.mode;
    this.draftImageIds = draft.imageIds;
    this.showNewSession = false;
  }

  async handleNewSessionSubmit(data: {
    prompt: string;
    mode: BranchSessionType;
    imageIds: string[];
    provider?: string;
    acpConfigSelection?: AcpConfigSelection | null;
  }) {
    this.newSessionMode = data.mode;
    this.showNewSession = false;
    this.draftPrompt = '';
    this.draftImageIds = [];

    const prompt =
      data.prompt.trim() === '' && data.mode === 'review'
        ? 'Review the code changes on this branch.'
        : data.prompt;
    void this.startOrQueueSession(data.mode, prompt, data.imageIds, {
      provider: data.provider,
      acpConfigSelection: data.acpConfigSelection,
    });
  }

  handleTimelineSessionClick(sessionId: string) {
    this.openSessionId = sessionId;
  }
}
