<script lang="ts">
  import { onMount } from 'svelte';
  import Send from '@lucide/svelte/icons/send';
  import Spinner from '../../shared/Spinner.svelte';
  import { Button } from '$lib/components/ui/button';
  import { toast } from 'svelte-sonner';
  import * as commands from '../../api/commands';
  import { getCommitPrefillFromReviewComments } from '../branches/commitSessionPrefill';
  import type { HashtagItem } from '../../types';
  import HashtagInput from '../sessions/HashtagInput.svelte';
  import { buildBranchHashtagItems } from '../sessions/hashtagItems';
  import { getPreferredAgent } from '../settings/preferences.svelte';
  import { agentState, REMOTE_AGENTS } from '../agents/agent.svelte';
  import { viewport } from '../../shared/viewport.svelte';
  import { onBranchSessionStatus } from '../../services/branchEventService';
  import { shouldQueueBranchSession } from '../branches/branchSessionQueue';

  interface Props {
    branchId: string;
    projectId?: string | null;
    commitSha: string;
    scope: 'branch' | 'commit';
    reviewId?: string;
    visibleCommentCount: number;
    githubRepo?: string;
    subpath?: string | null;
    isRemote: boolean;
    onStarted: () => void;
  }

  let {
    branchId,
    projectId,
    commitSha,
    scope,
    reviewId,
    visibleCommentCount,
    githubRepo,
    subpath,
    isRemote,
    onStarted,
  }: Props = $props();

  let draftPrompt = $state('');
  let isDirty = $state(false);
  let starting = $state(false);
  let timelineLoading = $state(true);
  let shouldQueueCommitSession = $state(true);
  let textareaElement = $state<HTMLElement | null>(null);

  // Hashtag reference items
  let hashtagItems = $state<HashtagItem[]>([]);
  $effect(() => {
    let stale = false;
    buildBranchHashtagItems(branchId, projectId ?? null, {
      repoSlug: githubRepo,
      repoSubpath: subpath,
    }).then((items) => {
      if (!stale) hashtagItems = items;
    });
    return () => {
      stale = true;
    };
  });

  const MIN_TEXTAREA_HEIGHT_PX = 80;
  const MAX_TEXTAREA_HEIGHT_PX = 260;

  let suggestedPrompt = $derived(getCommitPrefillFromReviewComments(visibleCommentCount));
  let willQueue = $derived(timelineLoading || shouldQueueCommitSession);

  $effect(() => {
    if (!isDirty) {
      draftPrompt = suggestedPrompt;
    }
  });

  $effect(() => {
    draftPrompt;
    syncTextareaHeight();
  });

  async function refreshQueueState(force = false): Promise<boolean> {
    try {
      const timeline = await commands.getBranchTimeline(branchId, { force });
      shouldQueueCommitSession = shouldQueueBranchSession({ mode: 'commit', timeline });
    } catch (e) {
      console.error('[DiffCommitSessionLauncher] Failed to load timeline:', e);
      shouldQueueCommitSession = true;
    } finally {
      timelineLoading = false;
    }
    return shouldQueueCommitSession;
  }

  onMount(() => {
    void refreshQueueState();

    document.addEventListener('keydown', handleGlobalKeydown);

    const unlisten = onBranchSessionStatus(branchId, () => {
      void refreshQueueState(true);
    });

    return () => {
      document.removeEventListener('keydown', handleGlobalKeydown);
      unlisten();
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
    let shouldQueue = true;
    try {
      shouldQueue = await refreshQueueState(true);

      const launchContext = {
        source: 'diff_viewer' as const,
        scope,
        commitSha,
        reviewId: reviewId ?? null,
      };

      const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
      const provider = getPreferredAgent(agents) ?? undefined;

      if (shouldQueue) {
        await commands.queueBranchSession(
          branchId,
          finalPrompt,
          'commit',
          provider,
          undefined,
          launchContext
        );
      } else {
        await commands.startBranchSession(
          branchId,
          finalPrompt,
          'commit',
          provider,
          undefined,
          launchContext
        );
      }

      onStarted();
    } catch (e) {
      toast.error(
        shouldQueue ? 'Unable to queue commit session' : 'Unable to start commit session',
        {
          description: e instanceof Error ? e.message : String(e),
          duration: Infinity,
        }
      );
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
    <Button
      type="button"
      variant="outline"
      class="w-full gap-1.5"
      onclick={handleSubmit}
      disabled={starting || !draftPrompt.trim()}
    >
      {#if starting}
        <Spinner size={14} />
        {willQueue ? 'Queueing…' : 'Starting…'}
      {:else}
        <Send size={14} />
        {willQueue ? 'Queue commit' : 'Start commit'}
        {#if viewport.showShortcutHints}
          <span class="shortcut-badge">⌘↵</span>
        {/if}
      {/if}
    </Button>
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

  .shortcut-badge {
    margin-left: auto;
    font-size: var(--size-xs);
    opacity: 0.6;
  }
</style>
