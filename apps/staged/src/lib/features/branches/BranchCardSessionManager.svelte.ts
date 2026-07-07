/**
 * BranchCardSessionManager — reactive session creation logic for BranchCard
 *
 * Manages new session modal state, auto review adoption/cancellation, and
 * branch-card session start orchestration.
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
import * as commands from '../../api/commands';
import { getPreferredAgent } from '../settings/preferences.svelte';
import { agentState, REMOTE_AGENTS } from '../agents/agent.svelte';
import { projectStateStore } from '../../stores/projectState.svelte';
import { sessionRegistry } from '../../stores/sessionRegistry.svelte';
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
  private setTimeline: (tl: BranchTimelineData) => void = undefined!;

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

  // Auto review state — tracks a background review started after each commit
  autoReviewSessionId = $state<string | null>(null);
  autoReviewId = $state<string | null>(null);
  // Tracks the session ID of an adopted auto-review so its completion event
  // can be ignored (it would otherwise trigger a spurious timeline reload).
  adoptedSessionId = $state<string | null>(null);

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
    setTimeline: (tl: BranchTimelineData) => void;
  }) {
    this.getBranch = opts.getBranch;
    this.getIsRemote = opts.getIsRemote;
    this.loadTimeline = opts.loadTimeline;
    this.getTimeline = opts.getTimeline;
    this.setTimeline = opts.setTimeline;
  }

  willQueueForMode(mode: BranchSessionType): boolean {
    return shouldQueueBranchSession({
      mode,
      timeline: this.getTimeline(),
      hasPendingSessionStart: this.isSessionStartPending,
      hasPendingQueuedSession: this.hasPendingQueuedSession,
    });
  }

  /** Register a session on the frontend and mark it as running. */
  private registerRunningSession(
    sessionId: string,
    projectId: string,
    mode: BranchSessionType,
    branchId: string
  ) {
    sessionRegistry.register(sessionId, projectId, mode, branchId);
    projectStateStore.addRunningSession(projectId, sessionId);
  }

  cancelAutoReview() {
    if (this.autoReviewSessionId) {
      commands.cancelSession(this.autoReviewSessionId).catch(() => {});
    }
    if (this.autoReviewId) {
      commands.deleteReview(this.autoReviewId).catch(() => {});
    }
    this.autoReviewSessionId = null;
    this.autoReviewId = null;
  }

  async tryAdoptAutoReview(): Promise<boolean> {
    if (this.hasCommitSessionInProgress) return false;

    const branch = this.getBranch();
    const isRemote = this.getIsRemote();

    try {
      const review = await commands.findFreshAutoReview(branch.id);
      if (!review) return false;

      // Check that the autoreview's agent matches the user's current
      // preferred agent. If they differ, skip adoption so a fresh review
      // is started with the correct agent instead.
      // A null reviewProvider means the session predates provider tracking —
      // treat it as compatible to avoid discarding valid reviews.
      const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
      const preferredAgent = getPreferredAgent(agents);
      const reviewProvider = review.sessionProvider ?? null;
      if (reviewProvider !== null && preferredAgent !== reviewProvider) {
        return false;
      }

      if (this.autoReviewSessionId) {
        // We're tracking the session locally — register it before revealing
        this.registerRunningSession(
          this.autoReviewSessionId,
          branch.projectId,
          'review',
          branch.id
        );
      } else if (!review.completedAt && review.sessionId) {
        // The autoreview has a session we're not tracking. Check its status
        // to decide whether to resume or just register it.
        const session = await commands.getSession(review.sessionId);
        if (session && session.status === 'running') {
          // Session is already running (e.g. agent connected but frontend
          // lost track) — just register it, no resume needed.
          this.registerRunningSession(review.sessionId, branch.projectId, 'review', branch.id);
        } else {
          // Session exists but isn't running — resume it
          await commands.resumeSession(
            review.sessionId,
            'Continue reviewing the code changes on this branch.',
            undefined,
            branch.id
          );
          this.registerRunningSession(review.sessionId, branch.projectId, 'review', branch.id);
        }
      }

      // Only reveal the review after all fallible operations succeed
      await commands.setReviewAuto(review.id, false);

      // Optimistically update the local timeline so the review is visible
      // immediately, before the backend reload completes.
      const currentTimeline = this.getTimeline();
      if (currentTimeline) {
        this.setTimeline({
          ...currentTimeline,
          reviews: currentTimeline.reviews.map((r) =>
            r.id === review.id ? { ...r, isAuto: false } : r
          ),
        });
      }

      this.adoptedSessionId = this.autoReviewSessionId;
      this.autoReviewSessionId = null;
      this.autoReviewId = null;

      this.loadTimeline();
      return true;
    } catch (e) {
      console.error('[BranchCard] Failed to adopt auto review:', e);
      return false;
    }
  }

  async startOrQueueSession(
    mode: BranchSessionType,
    prompt: string,
    imageIds: string[] = [],
    launchOptions: BranchSessionLaunchOptions = {}
  ) {
    const branch = this.getBranch();
    const isRemote = this.getIsRemote();

    if (this.autoReviewSessionId && mode !== 'note') {
      this.cancelAutoReview();
    }

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

    const adopted = await this.tryAdoptAutoReview();
    if (adopted) return;

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

    if (data.mode === 'review' && !data.prompt.trim()) {
      const adopted = await this.tryAdoptAutoReview();
      if (adopted) return;
      void this.startOrQueueSession(
        data.mode,
        'Review the code changes on this branch.',
        data.imageIds,
        {
          provider: data.provider,
          acpConfigSelection: data.acpConfigSelection,
        }
      );
      return;
    }

    const prompt =
      data.prompt || (data.mode === 'review' ? 'Review the code changes on this branch.' : '');
    void this.startOrQueueSession(data.mode, prompt, data.imageIds, {
      provider: data.provider,
      acpConfigSelection: data.acpConfigSelection,
    });
  }

  handleTimelineSessionClick(sessionId: string) {
    this.openSessionId = sessionId;
  }
}
