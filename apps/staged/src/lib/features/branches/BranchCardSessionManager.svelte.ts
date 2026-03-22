/**
 * BranchCardSessionManager — reactive session creation logic for BranchCard
 *
 * Manages new session modal state, pending session items, auto review
 * adoption/cancellation, and session start orchestration.
 *
 * Instantiated with a reactive branch reference. Exposes state as reactive
 * properties and methods. The parent calls prunePendingSessionItems() when
 * the timeline refreshes and reads pendingSessionItems to pass to BranchTimeline.
 */

import type { Branch, BranchTimeline as BranchTimelineData, BranchSessionType } from '../../types';
import * as commands from '../../api/commands';
import { getPreferredAgent } from '../settings/preferences.svelte';
import { agentState, REMOTE_AGENTS } from '../agents/agent.svelte';
import { alerts } from '../../shared/alerts.svelte';
import { projectStateStore } from '../../stores/projectState.svelte';
import { sessionRegistry } from '../../stores/sessionRegistry.svelte';

type PendingSessionItemType = 'pending-commit' | 'generating-note' | 'generating-review';

export type PendingSessionItem = {
  key: string;
  type: PendingSessionItemType;
  title: string;
  secondaryMeta?: string;
  sessionId?: string;
};

export default class BranchCardSessionManager {
  // Private callback refs — declared first so $derived fields can reference them
  private getBranch: () => Branch = undefined!;
  private getIsRemote: () => boolean = undefined!;
  private loadTimeline: () => void = undefined!;
  private getTimeline: () => BranchTimelineData | null = () => null;

  // New session modal state
  showNewSession = $state(false);
  newSessionMode = $state<BranchSessionType>('commit');
  draftPrompt = $state('');
  draftImageIds = $state<string[]>([]);
  pendingSessionItems = $state<PendingSessionItem[]>([]);
  isSessionStartPending = $derived(this.pendingSessionItems.some((item) => !item.sessionId));

  // Auto review state — tracks a background review started after each commit
  autoReviewSessionId = $state<string | null>(null);
  autoReviewId = $state<string | null>(null);

  // Session modal (opened after starting a branch session, or from timeline)
  openSessionId = $state<string | null>(null);

  /** True when any session is actively generating on this branch's timeline. */
  hasRunningSession = $derived.by(() => {
    const tl = this.getTimeline();
    if (!tl) return false;
    return (
      tl.commits.some((c) => c.sessionStatus === 'running') ||
      tl.notes.some((n) => n.sessionStatus === 'running') ||
      tl.reviews.some((r) => r.sessionStatus === 'running')
    );
  });

  /** True when a new session will be queued rather than started immediately. */
  willQueue = $derived(
    !this.getTimeline() || // provisioning — no timeline yet
      this.hasRunningSession // another session is active
  );

  /** True when new session actions (new commit, note, review) should be disabled. */
  isNewSessionDisabled = $derived(this.showNewSession || this.isSessionStartPending);

  constructor(opts: {
    getBranch: () => Branch;
    getIsRemote: () => boolean;
    loadTimeline: () => void;
    getTimeline: () => BranchTimelineData | null;
  }) {
    this.getBranch = opts.getBranch;
    this.getIsRemote = opts.getIsRemote;
    this.loadTimeline = opts.loadTimeline;
    this.getTimeline = opts.getTimeline;
  }

  prunePendingSessionItems(nextTimeline: BranchTimelineData): Set<string> {
    const persistedSessionIds = new Set<string>();
    for (const commit of nextTimeline.commits) {
      if (commit.sessionId) persistedSessionIds.add(commit.sessionId);
    }
    for (const note of nextTimeline.notes) {
      if (note.sessionId) persistedSessionIds.add(note.sessionId);
    }
    for (const review of nextTimeline.reviews) {
      if (review.sessionId) persistedSessionIds.add(review.sessionId);
    }

    const prunedSessionIds = new Set<string>();
    this.pendingSessionItems = this.pendingSessionItems.filter((item) => {
      if (item.sessionId && persistedSessionIds.has(item.sessionId)) {
        prunedSessionIds.add(item.sessionId);
        return false;
      }
      return true;
    });
    return prunedSessionIds;
  }

  private pendingSessionTypeForMode(mode: BranchSessionType): PendingSessionItemType {
    switch (mode) {
      case 'note':
        return 'generating-note';
      case 'review':
        return 'generating-review';
      default:
        return 'pending-commit';
    }
  }

  private pendingSessionTitleForMode(mode: BranchSessionType, prompt: string): string {
    const trimmed = prompt.trim();
    if (mode === 'review') return 'Code Review';
    if (trimmed) return trimmed;
    return mode === 'note' ? 'Untitled note' : 'Pending commit';
  }

  private pendingSessionMetaForMode(mode: BranchSessionType): string {
    switch (mode) {
      case 'note':
        return 'Starting note session...';
      case 'review':
        return 'Starting review session...';
      default:
        return 'Starting commit session...';
    }
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
    const branch = this.getBranch();

    try {
      const review = await commands.findFreshAutoReview(branch.id);
      if (!review) return false;

      await commands.setReviewAuto(review.id, false);

      if (this.autoReviewSessionId) {
        sessionRegistry.register(this.autoReviewSessionId, branch.projectId, 'review', branch.id);
        projectStateStore.addRunningSession(branch.projectId, this.autoReviewSessionId);
      }

      this.autoReviewSessionId = null;
      this.autoReviewId = null;

      this.loadTimeline();
      return true;
    } catch (e) {
      console.error('[BranchCard] Failed to adopt auto review:', e);
      return false;
    }
  }

  async startBranchSessionWithPendingItem(
    mode: BranchSessionType,
    prompt: string,
    imageIds: string[] = []
  ) {
    const branch = this.getBranch();
    const isRemote = this.getIsRemote();

    if (this.autoReviewSessionId && mode !== 'review') {
      this.cancelAutoReview();
    }

    const pendingKey = `session-start-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    this.pendingSessionItems = [
      ...this.pendingSessionItems,
      {
        key: pendingKey,
        type: this.pendingSessionTypeForMode(mode),
        title: this.pendingSessionTitleForMode(mode, prompt),
        secondaryMeta: this.pendingSessionMetaForMode(mode),
      },
    ];

    try {
      const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
      const result = await commands.startBranchSession(
        branch.id,
        prompt,
        mode,
        getPreferredAgent(agents) ?? undefined,
        imageIds.length > 0 ? imageIds : undefined
      );

      if (!result || !result.sessionId) {
        throw new Error('Failed to start session: no session ID returned');
      }

      sessionRegistry.register(result.sessionId, branch.projectId, mode, branch.id);
      projectStateStore.addRunningSession(branch.projectId, result.sessionId);

      this.pendingSessionItems = this.pendingSessionItems.map((item) =>
        item.key === pendingKey
          ? {
              ...item,
              sessionId: result.sessionId,
              secondaryMeta: undefined,
            }
          : item
      );

      this.loadTimeline();
    } catch (e) {
      this.pendingSessionItems = this.pendingSessionItems.filter((item) => item.key !== pendingKey);
      alerts.show({
        tone: 'error',
        title: 'Unable to start session',
        message: e instanceof Error ? e.message : String(e),
        durationMs: 0,
      });
    }
  }

  async queueBranchSession(mode: BranchSessionType, prompt: string, imageIds: string[] = []) {
    const branch = this.getBranch();
    const isRemote = this.getIsRemote();

    if (this.autoReviewSessionId && mode !== 'review') {
      this.cancelAutoReview();
    }

    const pendingKey = `session-queue-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    const queueMeta = this.getTimeline()
      ? 'Queued \u2014 waiting for current session\u2026'
      : 'Queued \u2014 waiting for workspace\u2026';

    this.pendingSessionItems = [
      ...this.pendingSessionItems,
      {
        key: pendingKey,
        type: this.pendingSessionTypeForMode(mode),
        title: this.pendingSessionTitleForMode(mode, prompt),
        secondaryMeta: queueMeta,
      },
    ];

    try {
      const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
      const result = await commands.queueBranchSession(
        branch.id,
        prompt,
        mode,
        getPreferredAgent(agents) ?? undefined,
        imageIds.length > 0 ? imageIds : undefined
      );

      if (!result || !result.sessionId) {
        throw new Error('Failed to queue session: no session ID returned');
      }

      this.pendingSessionItems = this.pendingSessionItems.map((item) =>
        item.key === pendingKey
          ? {
              ...item,
              sessionId: result.sessionId,
            }
          : item
      );

      this.loadTimeline();
    } catch (e) {
      this.pendingSessionItems = this.pendingSessionItems.filter((item) => item.key !== pendingKey);
      alerts.show({
        tone: 'error',
        title: 'Unable to queue session',
        message: e instanceof Error ? e.message : String(e),
        durationMs: 0,
      });
    }
  }

  async startOrQueueSession(mode: BranchSessionType, prompt: string, imageIds: string[] = []) {
    if (this.willQueue) {
      await this.queueBranchSession(mode, prompt, imageIds);
    } else {
      await this.startBranchSessionWithPendingItem(mode, prompt, imageIds);
    }
  }

  openNewSession(mode: BranchSessionType, e?: MouseEvent) {
    if (mode === 'review' && e?.altKey) {
      void this.startReviewSessionImmediately();
      return;
    }
    this.newSessionMode = mode;
    this.showNewSession = true;
  }

  async startReviewSessionImmediately() {
    this.newSessionMode = 'review';
    this.showNewSession = false;
    this.draftPrompt = '';
    this.draftImageIds = [];

    const adopted = await this.tryAdoptAutoReview();
    if (adopted) return;

    const reviewPrompt = 'Review the code changes on this branch.';
    await this.startBranchSessionWithPendingItem('review', reviewPrompt);
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
        data.imageIds
      );
      return;
    }

    if (data.mode === 'review' && this.autoReviewSessionId) {
      this.cancelAutoReview();
    }

    const prompt =
      data.prompt || (data.mode === 'review' ? 'Review the code changes on this branch.' : '');
    void this.startOrQueueSession(data.mode, prompt, data.imageIds);
  }

  handleTimelineSessionClick(sessionId: string) {
    this.openSessionId = sessionId;
  }
}
