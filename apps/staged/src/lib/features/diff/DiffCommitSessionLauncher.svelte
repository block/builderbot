<script lang="ts">
  import { onMount } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { Send } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import { alerts } from '../../shared/alerts.svelte';
  import * as commands from '../../api/commands';
  import { getCommitPrefillFromReviewComments } from '../branches/commitSessionPrefill';
  import type { SessionStatusPayload, HashtagItem } from '../../types';
  import HashtagInput from '../sessions/HashtagInput.svelte';
  import { buildBranchHashtagItems } from '../sessions/hashtagItems';

  interface Props {
    branchId: string;
    projectId?: string | null;
    commitSha: string;
    scope: 'branch' | 'commit';
    reviewId?: string;
    visibleCommentCount: number;
    onStarted: () => void;
  }

  let { branchId, projectId, commitSha, scope, reviewId, visibleCommentCount, onStarted }: Props =
    $props();

  let draftPrompt = $state('');
  let isDirty = $state(false);
  let starting = $state(false);
  let timelineLoading = $state(true);
  let hasRunningSession = $state(false);
  let textareaElement = $state<HTMLElement | null>(null);

  // Hashtag reference items
  let hashtagItems = $state<HashtagItem[]>([]);
  $effect(() => {
    let stale = false;
    buildBranchHashtagItems(branchId, projectId ?? null).then((items) => {
      if (!stale) hashtagItems = items;
    });
    return () => {
      stale = true;
    };
  });

  const MIN_TEXTAREA_HEIGHT_PX = 80;
  const MAX_TEXTAREA_HEIGHT_PX = 260;

  let suggestedPrompt = $derived(getCommitPrefillFromReviewComments(visibleCommentCount));
  let willQueue = $derived(timelineLoading || hasRunningSession);

  $effect(() => {
    if (!isDirty) {
      draftPrompt = suggestedPrompt;
    }
  });

  $effect(() => {
    draftPrompt;
    syncTextareaHeight();
  });

  async function refreshQueueState(force = false) {
    try {
      const timeline = await commands.getBranchTimeline(branchId, { force });
      hasRunningSession =
        timeline.commits.some(
          (c) => c.sessionStatus === 'running' || c.sessionStatus === 'queued'
        ) ||
        timeline.notes.some((n) => n.sessionStatus === 'running' || n.sessionStatus === 'queued') ||
        timeline.reviews.some(
          (r) => !r.isAuto && (r.sessionStatus === 'running' || r.sessionStatus === 'queued')
        );
    } catch (e) {
      console.error('[DiffCommitSessionLauncher] Failed to load timeline:', e);
      hasRunningSession = false;
    } finally {
      timelineLoading = false;
    }
  }

  onMount(() => {
    void refreshQueueState();

    document.addEventListener('keydown', handleGlobalKeydown);

    let unlisten: UnlistenFn | null = null;
    listen<SessionStatusPayload>('session-status-changed', (event) => {
      if (event.payload.branchId !== branchId) return;
      void refreshQueueState(true);
    }).then((fn) => {
      unlisten = fn;
    });

    return () => {
      document.removeEventListener('keydown', handleGlobalKeydown);
      unlisten?.();
    };
  });

  function handleInput(_event: Event) {
    isDirty = true;
    syncTextareaHeight();
  }

  function handleGlobalKeydown(e: KeyboardEvent) {
    if (e.key !== 'Enter' || !e.metaKey) return;
    if (starting || !draftPrompt.trim()) return;

    const target = e.target as HTMLElement | null;
    const inInput =
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement ||
      (target instanceof HTMLElement && target.isContentEditable);

    // Allow if focus is in the commit textbox itself, or not in any input
    if (inInput && !textareaElement?.contains(target)) return;

    e.preventDefault();
    handleSubmit();
  }

  function syncTextareaHeight() {
    if (!textareaElement) return;

    textareaElement.style.height = 'auto';
    textareaElement.style.height = `${Math.min(
      Math.max(textareaElement.scrollHeight, MIN_TEXTAREA_HEIGHT_PX),
      MAX_TEXTAREA_HEIGHT_PX
    )}px`;
  }

  async function handleSubmit() {
    let finalPrompt = draftPrompt.trim();
    if (!finalPrompt || starting) return;

    // Prepend a reference to the review when launched from a review context
    if (reviewId) {
      finalPrompt = `Re: #review:${reviewId}\n${finalPrompt}`;
    }

    starting = true;
    try {
      await refreshQueueState(true);

      const launchContext = {
        source: 'diff_viewer' as const,
        scope,
        commitSha,
        reviewId: reviewId ?? null,
      };

      if (hasRunningSession) {
        await commands.queueBranchSession(
          branchId,
          finalPrompt,
          'commit',
          undefined,
          undefined,
          launchContext
        );
      } else {
        await commands.startBranchSession(
          branchId,
          finalPrompt,
          'commit',
          undefined,
          undefined,
          launchContext
        );
      }

      onStarted();
    } catch (e) {
      alerts.show({
        tone: 'error',
        title: hasRunningSession
          ? 'Unable to queue commit session'
          : 'Unable to start commit session',
        message: e instanceof Error ? e.message : String(e),
        durationMs: 0,
      });
    } finally {
      starting = false;
    }
  }
</script>

<div class="composer">
  <HashtagInput
    bind:textareaEl={textareaElement}
    bind:value={draftPrompt}
    class="composer-input"
    placeholder="Describe any changes you want"
    rows="3"
    disabled={starting}
    oninput={handleInput}
    items={hashtagItems}
  />
  <div class="composer-footer">
    <button
      class="composer-submit"
      type="button"
      onclick={handleSubmit}
      disabled={starting || !draftPrompt.trim()}
    >
      {#if starting}
        <Spinner size={14} />
        {willQueue ? 'Queueing…' : 'Starting…'}
      {:else}
        <Send size={14} />
        {willQueue ? 'Queue commit' : 'Start commit'}
        <span class="shortcut-badge">⌘↵</span>
      {/if}
    </button>
  </div>
</div>

<style>
  .composer {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    border-top: 1px solid var(--border-subtle);
    background: color-mix(in srgb, var(--bg-chrome) 94%, var(--bg-hover));
  }

  .composer :global(.composer-input) {
    padding: 10px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-md);
    font-family: inherit;
    line-height: 1.5;
    resize: none;
    overflow-y: auto;
    min-height: 80px;
    max-height: 260px;
    transition: border-color 0.15s;
  }

  .composer :global(.composer-input):focus {
    outline: none;
    border-color: var(--border-emphasis);
  }

  .composer-footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
  }

  .composer-submit {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 8px 16px;
    border-radius: 6px;
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
    background: var(--ui-accent);
    border: none;
    color: var(--bg-deepest);
    width: 100%;
  }

  .shortcut-badge {
    margin-left: auto;
    font-size: var(--size-xs);
    opacity: 0.6;
  }

  .composer-submit:hover:not(:disabled) {
    background: var(--ui-accent-hover);
  }

  .composer-submit:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
