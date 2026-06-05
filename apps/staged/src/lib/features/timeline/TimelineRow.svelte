<!--
  TimelineRow.svelte - Renders a single timeline item (commit, note, or review)

  Icon + title + meta. Compact. The whole row is clickable to view the item.
  Hover reveals session and delete actions on the right.
-->
<script lang="ts">
  import GitCommitVertical from '@lucide/svelte/icons/git-commit-vertical';
  import FileDiff from '@lucide/svelte/icons/file-diff';
  import FileText from '@lucide/svelte/icons/file-text';
  import FileSearch from '@lucide/svelte/icons/file-search';
  import ImageLucide from '@lucide/svelte/icons/image';
  import MessageSquare from '@lucide/svelte/icons/message-square';
  import MessageSquarePlus from '@lucide/svelte/icons/message-square-plus';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import AlertTriangle from '@lucide/svelte/icons/alert-triangle';
  import Clock from '@lucide/svelte/icons/clock';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import GitMerge from '@lucide/svelte/icons/git-merge';
  import ChevronsDown from '@lucide/svelte/icons/chevrons-down';
  import Copy from '@lucide/svelte/icons/copy';
  import Spinner from '../../shared/Spinner.svelte';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import { Button } from '$lib/components/ui/button';
  import * as Tooltip from '$lib/components/ui/tooltip';

  export type TimelineItemType =
    | 'commit'
    | 'pending-commit'
    | 'queued-commit'
    | 'failed-commit'
    | 'note'
    | 'generating-note'
    | 'queued-note'
    | 'failed-note'
    | 'review'
    | 'generating-review'
    | 'queued-review'
    | 'failed-review'
    | 'image'
    | 'git-info'
    | 'git-warning'
    | 'git-merge'
    | 'git-merge-warning'
    | 'git-pull'
    | 'git-push'
    | 'git-diff'
    | 'provisioning'
    | 'load-error';

  export type TimelineBadge = {
    icon: 'comment' | 'warning';
    count: number;
  };

  interface Props {
    type: TimelineItemType;
    title: string;
    /** Pre-rendered HTML title with hashtag badges. When set, takes precedence over `title`. */
    titleHtml?: string;
    meta?: string;
    secondaryMeta?: string;
    /** Tertiary metadata shown after the secondary meta (e.g. commit author). */
    tertiaryMeta?: string;
    badges?: TimelineBadge[];
    deleting?: boolean;
    isLast?: boolean;
    sessionId?: string;
    onItemClick?: () => void;
    onSessionClick?: (sessionId: string) => void;
    onDeleteClick?: (opts?: { altKey: boolean }) => void;
    /** When set, the delete button is shown but disabled with this tooltip. */
    deleteDisabledReason?: string;
    onRetryClick?: () => void;
    onStartClick?: () => void;
    onResumeClick?: () => void;
    onPullClick?: () => void;
    pullDisabledReason?: string;
    onPushClick?: () => void;
    pushDisabledReason?: string;
    onRebaseClick?: () => void;
    rebaseDisabledReason?: string;
    onForcePushClick?: () => void;
    forcePushDisabledReason?: string;
    forcePushing?: boolean;
    pushing?: boolean;
    onViewDiffClick?: () => void;
    onCommitChangesClick?: () => void;
    commitChangesDisabledReason?: string;
    onDiscardChangesClick?: () => void;
    discardChangesDisabledReason?: string;
    showConnector?: boolean;
    /** Full commit SHA for the context menu "Copy SHA" action. */
    commitSha?: string;
    /** Hashtag reference token (e.g. "#commit:abc123") for "New session referring to this". */
    hashtagRef?: string;
    /** Callback invoked when the user picks "New session referring to this" from the context menu. */
    onNewSessionReferring?: (hashtagRef: string) => void;
  }

  let {
    type,
    title,
    titleHtml,
    meta,
    secondaryMeta,
    tertiaryMeta,
    badges,
    deleting = false,
    isLast = false,
    sessionId,
    onItemClick,
    onSessionClick,
    onDeleteClick,
    deleteDisabledReason,
    onRetryClick,
    onStartClick,
    onResumeClick,
    onPullClick,
    pullDisabledReason,
    onPushClick,
    pushDisabledReason,
    onRebaseClick,
    rebaseDisabledReason,
    onForcePushClick,
    forcePushDisabledReason,
    forcePushing = false,
    pushing = false,
    onViewDiffClick,
    onCommitChangesClick,
    commitChangesDisabledReason,
    onDiscardChangesClick,
    discardChangesDisabledReason,
    showConnector = true,
    commitSha,
    hashtagRef,
    onNewSessionReferring,
  }: Props = $props();

  let isNote = $derived(
    type === 'note' ||
      type === 'generating-note' ||
      type === 'queued-note' ||
      type === 'failed-note'
  );
  let isReview = $derived(
    type === 'review' ||
      type === 'generating-review' ||
      type === 'queued-review' ||
      type === 'failed-review'
  );
  let isImage = $derived(type === 'image');
  let isGitState = $derived(
    type === 'git-info' ||
      type === 'git-warning' ||
      type === 'git-merge' ||
      type === 'git-merge-warning' ||
      type === 'git-pull' ||
      type === 'git-push' ||
      type === 'git-diff'
  );
  let isQueued = $derived(
    type === 'queued-commit' || type === 'queued-note' || type === 'queued-review'
  );
  let isPending = $derived(
    deleting ||
      isQueued ||
      type === 'pending-commit' ||
      type === 'generating-note' ||
      type === 'generating-review' ||
      type === 'provisioning'
  );
  let isFailed = $derived(
    !deleting &&
      (type === 'failed-commit' ||
        type === 'failed-note' ||
        type === 'failed-review' ||
        type === 'load-error')
  );
  let isClickable = $derived(!!onItemClick && !isPending && !isFailed);
  let hasSession = $derived(!!sessionId && !deleting);

  function handleRowClick() {
    if (isClickable) {
      onItemClick?.();
    }
  }

  function handleSessionClick(e: MouseEvent) {
    e.stopPropagation();
    if (sessionId && onSessionClick) {
      onSessionClick(sessionId);
    }
  }

  function handleDeleteClick(e: MouseEvent) {
    e.stopPropagation();
    onDeleteClick?.({ altKey: e.altKey });
  }

  function handleRetryClick(e: MouseEvent) {
    e.stopPropagation();
    onRetryClick?.();
  }

  function handleStartClick(e: MouseEvent) {
    e.stopPropagation();
    onStartClick?.();
  }

  function handleResumeClick(e: MouseEvent) {
    e.stopPropagation();
    onResumeClick?.();
  }

  function handlePullClick(e: MouseEvent) {
    e.stopPropagation();
    onPullClick?.();
  }

  function handlePushClick(e: MouseEvent) {
    e.stopPropagation();
    onPushClick?.();
  }

  function handleRebaseClick(e: MouseEvent) {
    e.stopPropagation();
    onRebaseClick?.();
  }

  function handleForcePushClick(e: MouseEvent) {
    e.stopPropagation();
    onForcePushClick?.();
  }

  function handleViewDiffClick(e: MouseEvent) {
    e.stopPropagation();
    onViewDiffClick?.();
  }

  function handleCommitChangesClick(e: MouseEvent) {
    e.stopPropagation();
    onCommitChangesClick?.();
  }

  function handleDiscardChangesClick(e: MouseEvent) {
    e.stopPropagation();
    onDiscardChangesClick?.();
  }

  // ── Context menu ────────────────────────────────────────────────────
  let hasContextMenu = $derived(!!commitSha || (!!hashtagRef && !!onNewSessionReferring));
</script>

{#snippet rowBody()}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="timeline-row"
    class:pending={isPending}
    class:failed={isFailed}
    class:clickable={isClickable}
    class:git-state={isGitState}
    class:compact={type === 'load-error'}
    onclick={handleRowClick}
  >
    <div class="timeline-marker">
      <div
        class="timeline-icon"
        class:commit-icon={type === 'commit' ||
          type === 'pending-commit' ||
          type === 'queued-commit'}
        class:note-icon={type === 'note' || type === 'generating-note' || type === 'queued-note'}
        class:review-icon={type === 'review' ||
          type === 'generating-review' ||
          type === 'queued-review'}
        class:image-icon={isImage}
        class:branch-icon={isGitState}
        class:warning-icon={type === 'git-warning' || type === 'git-merge-warning'}
        class:failed-icon={isFailed}
      >
        {#if isQueued}
          <Clock size={12} />
        {:else if isPending}
          <Spinner size={12} />
        {:else if isFailed}
          <AlertTriangle size={12} />
        {:else if type === 'git-warning'}
          <AlertTriangle size={12} />
        {:else if type === 'git-merge' || type === 'git-merge-warning'}
          <GitMerge size={12} />
        {:else if type === 'git-pull'}
          <ChevronsDown size={12} />
        {:else if type === 'git-push'}
          <ChevronsDown size={12} />
        {:else if type === 'git-diff'}
          <FileDiff size={12} />
        {:else if type === 'commit'}
          <GitCommitVertical size={12} />
        {:else if isNote}
          <FileText size={12} />
        {:else if isReview}
          <FileSearch size={12} />
        {:else if isImage}
          <ImageLucide size={12} />
        {:else if isGitState}
          <GitBranch size={12} />
        {/if}
      </div>
      {#if showConnector && !isLast}
        <div class="timeline-line"></div>
      {/if}
    </div>
    <div class="timeline-content">
      <div class="timeline-info">
        {#if titleHtml}
          <span
            class="timeline-title"
            class:skeleton-title={isPending}
            class:failed-title={isFailed}>{@html titleHtml}</span
          >
        {:else}
          <span
            class="timeline-title"
            class:skeleton-title={isPending}
            class:failed-title={isFailed}>{title}</span
          >
        {/if}
        {#if meta || secondaryMeta || tertiaryMeta || (badges && badges.length > 0)}
          <div class="timeline-meta">
            {#if meta}
              <Tooltip.Root>
                <Tooltip.Trigger>
                  {#snippet child({ props })}
                    <span class="meta-item" {...props}>{meta}</span>
                  {/snippet}
                </Tooltip.Trigger>
                <Tooltip.Content>{meta}</Tooltip.Content>
              </Tooltip.Root>
            {/if}
            {#if secondaryMeta}
              <Tooltip.Root>
                <Tooltip.Trigger>
                  {#snippet child({ props })}
                    <span class="meta-item meta-sha" class:failed-meta={isFailed} {...props}
                      >{secondaryMeta}</span
                    >
                  {/snippet}
                </Tooltip.Trigger>
                <Tooltip.Content>{secondaryMeta}</Tooltip.Content>
              </Tooltip.Root>
            {/if}
            {#if tertiaryMeta}
              <Tooltip.Root>
                <Tooltip.Trigger>
                  {#snippet child({ props })}
                    <span class="meta-item" {...props}>{tertiaryMeta}</span>
                  {/snippet}
                </Tooltip.Trigger>
                <Tooltip.Content>{tertiaryMeta}</Tooltip.Content>
              </Tooltip.Root>
            {/if}
            {#if badges}
              {#each badges as badge}
                <span class="meta-badge">
                  {#if badge.icon === 'warning'}
                    <AlertTriangle size={10} />
                  {:else}
                    <MessageSquare size={10} />
                  {/if}
                  <span>{badge.count}</span>
                </span>
              {/each}
            {/if}
          </div>
        {/if}
      </div>
      <div
        class="timeline-actions"
        class:always-visible={!!onRetryClick ||
          !!onStartClick ||
          !!onResumeClick ||
          !!onPullClick ||
          !!pullDisabledReason ||
          !!onPushClick ||
          !!pushDisabledReason ||
          !!onRebaseClick ||
          !!rebaseDisabledReason ||
          !!onForcePushClick ||
          !!forcePushDisabledReason ||
          !!onViewDiffClick ||
          !!onCommitChangesClick ||
          !!commitChangesDisabledReason ||
          !!onDiscardChangesClick ||
          !!discardChangesDisabledReason}
      >
        {#if onStartClick}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="outline"
                  size="xs"
                  onclick={handleStartClick}
                  class="h-[22px] rounded border-[var(--border-muted)] bg-transparent text-[var(--text-muted)] shadow-none hover:border-[var(--border-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
                >
                  Start
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Start</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if onRetryClick}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="ghost"
                  size="xs"
                  onclick={handleRetryClick}
                  class="h-[22px] text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
                >
                  Retry
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Retry</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if onResumeClick}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="outline"
                  size="xs"
                  onclick={handleResumeClick}
                  class="h-[22px] rounded-md border-[var(--border-subtle)] bg-transparent text-[var(--text-muted)] shadow-none hover:border-[var(--border-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
                >
                  Resume
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>Resume session</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if onPullClick || pullDisabledReason}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <span {...props} class="inline-flex">
                  <Button
                    variant="outline"
                    size="xs"
                    onclick={handlePullClick}
                    disabled={!!pullDisabledReason}
                    class="h-[22px] rounded-md border-[var(--border-subtle)] bg-transparent text-[var(--text-muted)] shadow-none hover:border-[var(--border-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
                  >
                    Pull
                  </Button>
                </span>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>{pullDisabledReason ?? 'Pull'}</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if onPushClick || pushDisabledReason}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <span {...props} class="inline-flex">
                  <Button
                    variant="outline"
                    size="xs"
                    onclick={handlePushClick}
                    disabled={!!pushDisabledReason}
                    class="h-[22px] rounded-md border-[var(--border-subtle)] bg-transparent text-[var(--text-muted)] shadow-none hover:border-[var(--border-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
                  >
                    {pushing ? 'Pushing\u2026' : 'Push'}
                  </Button>
                </span>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>
              {pushDisabledReason ?? (pushing ? 'View push session' : 'Push')}
            </Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if onForcePushClick || forcePushDisabledReason}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <span {...props} class="inline-flex">
                  <Button
                    variant="outline"
                    size="xs"
                    onclick={handleForcePushClick}
                    disabled={!!forcePushDisabledReason}
                    class={[
                      'h-[22px] rounded-md bg-transparent shadow-none',
                      forcePushing
                        ? 'border-[var(--border-subtle)] text-[var(--text-muted)] hover:border-[var(--border-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground'
                        : 'border-[var(--ui-danger-bg)] font-medium text-[var(--ui-danger)] hover:border-[var(--ui-danger)] hover:bg-[var(--ui-danger-bg)] hover:text-[var(--ui-danger)]',
                    ]}
                  >
                    {forcePushing ? 'Pushing\u2026' : 'Force Push'}
                  </Button>
                </span>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>
              {forcePushDisabledReason ??
                (forcePushing ? 'View push session' : 'Force push local branch to origin')}
            </Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if onRebaseClick || rebaseDisabledReason}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <span {...props} class="inline-flex">
                  <Button
                    variant="outline"
                    size="xs"
                    onclick={handleRebaseClick}
                    disabled={!!rebaseDisabledReason}
                    class="h-[22px] rounded-md border-[var(--border-subtle)] bg-transparent text-[var(--text-muted)] shadow-none hover:border-[var(--border-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
                  >
                    Rebase
                  </Button>
                </span>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>{rebaseDisabledReason ?? 'Rebase'}</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if onViewDiffClick}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="outline"
                  size="xs"
                  onclick={handleViewDiffClick}
                  class="h-[22px] rounded-md border-[var(--border-subtle)] bg-transparent text-[var(--text-muted)] shadow-none hover:border-[var(--border-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
                >
                  Diff
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>View diff</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if onCommitChangesClick || commitChangesDisabledReason}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <span {...props} class="inline-flex">
                  <Button
                    variant="outline"
                    size="xs"
                    onclick={handleCommitChangesClick}
                    disabled={!!commitChangesDisabledReason}
                    class="h-[22px] rounded-md border-[var(--border-subtle)] bg-transparent text-[var(--text-muted)] shadow-none hover:border-[var(--border-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
                  >
                    Commit
                  </Button>
                </span>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>{commitChangesDisabledReason ?? 'Commit changes'}</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if onDiscardChangesClick || discardChangesDisabledReason}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <span {...props} class="inline-flex">
                  <Button
                    variant="outline"
                    size="xs"
                    onclick={handleDiscardChangesClick}
                    disabled={!!discardChangesDisabledReason}
                    class="h-[22px] rounded-md border-[var(--border-subtle)] bg-transparent text-[var(--text-muted)] shadow-none hover:border-[var(--border-muted)] hover:bg-[var(--bg-hover)] hover:text-foreground"
                  >
                    Discard
                  </Button>
                </span>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>{discardChangesDisabledReason ?? 'Discard changes'}</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if hasSession && !onStartClick && !isQueued}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  variant="ghost"
                  size="icon-xs"
                  onclick={handleSessionClick}
                  class="size-[22px] text-[var(--text-faint)] hover:bg-[var(--bg-hover)] hover:text-[var(--ui-accent)] [&_svg]:!size-3"
                >
                  <MessageSquare size={12} />
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>View session</Tooltip.Content>
          </Tooltip.Root>
        {/if}
        {#if onDeleteClick || deleteDisabledReason}
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <span {...props} class="inline-flex">
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    onclick={handleDeleteClick}
                    disabled={!!deleteDisabledReason}
                    class="size-[22px] text-[var(--text-faint)] hover:bg-[var(--bg-hover)] hover:text-destructive [&_svg]:!size-3"
                  >
                    <Trash2 size={12} />
                  </Button>
                </span>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>{deleteDisabledReason ?? 'Delete'}</Tooltip.Content>
          </Tooltip.Root>
        {/if}
      </div>
    </div>
  </div>
{/snippet}

{#if hasContextMenu}
  <ContextMenu.Root>
    <ContextMenu.Trigger class="contents">
      {@render rowBody()}
    </ContextMenu.Trigger>
    <ContextMenu.Content class="min-w-[140px]">
      {#if commitSha}
        <ContextMenu.Item
          onSelect={() => navigator.clipboard.writeText(commitSha!).catch(() => {})}
        >
          <Copy size={14} /> Copy SHA
        </ContextMenu.Item>
      {/if}
      {#if hashtagRef && onNewSessionReferring}
        <ContextMenu.Item onSelect={() => onNewSessionReferring!(hashtagRef!)}>
          <MessageSquarePlus size={14} /> New session referring to this
        </ContextMenu.Item>
      {/if}
    </ContextMenu.Content>
  </ContextMenu.Root>
{:else}
  {@render rowBody()}
{/if}

<style>
  .timeline-row {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 8px;
    margin: 0 -8px;
    border-radius: 6px;
    position: relative;
    transition: background-color 0.15s ease;
    will-change: transform;
  }

  .timeline-row:hover {
    background-color: var(--bg-hover);
  }

  .timeline-row.clickable {
    cursor: pointer;
  }

  .timeline-row.pending {
    cursor: default;
  }

  .timeline-row.compact {
    padding: 6px 8px;
  }

  .timeline-row.failed {
    cursor: default;
  }

  .timeline-marker {
    display: flex;
    flex-direction: column;
    align-items: center;
    width: 20px;
    flex-shrink: 0;
  }

  .timeline-line {
    flex: 1;
    width: 2px;
    min-height: 20px;
    background-color: var(--border-subtle);
    margin-top: 6px;
  }

  .timeline-row.git-state .timeline-line {
    flex: none;
    height: 6px;
    min-height: 0;
    margin-top: 6px;
  }

  .timeline-content {
    flex: 1;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    min-width: 0;
  }

  .timeline-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: 4px;
    flex-shrink: 0;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
  }

  .timeline-icon.commit-icon {
    color: var(--commit-color);
    background-color: var(--commit-bg);
    border-color: transparent;
  }

  .timeline-icon.note-icon {
    color: var(--note-color);
    background-color: var(--note-bg);
    border-color: transparent;
  }

  .timeline-icon.review-icon {
    color: var(--review-color);
    background-color: var(--review-bg);
    border-color: transparent;
  }

  .timeline-icon.image-icon {
    color: var(--image-color);
    background-color: var(--image-bg);
    border-color: transparent;
  }

  .timeline-icon.branch-icon {
    color: var(--text-muted);
    background-color: var(--bg-hover);
    border-color: transparent;
  }

  .timeline-icon.warning-icon {
    color: var(--ui-danger);
    background-color: var(--ui-danger-bg);
    border-color: transparent;
  }

  .timeline-row.pending .timeline-icon.commit-icon {
    background-color: var(--commit-bg);
    border-color: transparent;
  }

  .timeline-row.pending .timeline-icon.commit-icon :global(.spinner) {
    color: var(--commit-color);
  }

  .timeline-row.pending .timeline-icon.note-icon {
    background-color: var(--note-bg);
    border-color: transparent;
  }

  .timeline-row.pending .timeline-icon.note-icon :global(.spinner) {
    color: var(--note-color);
  }

  .timeline-row.pending .timeline-icon.review-icon {
    background-color: var(--review-bg);
    border-color: transparent;
  }

  .timeline-row.pending .timeline-icon.review-icon :global(.spinner) {
    color: var(--review-color);
  }

  .timeline-row.compact .timeline-icon {
    background-color: var(--bg-hover);
    border-color: var(--bg-hover);
  }

  .timeline-row.compact .timeline-icon :global(.spinner) {
    color: var(--text-faint);
  }

  .timeline-row.compact .timeline-title {
    color: var(--text-faint);
    font-weight: normal;
  }

  .timeline-icon.failed-icon {
    color: var(--text-muted);
    border-color: var(--border-muted);
  }

  .timeline-info {
    flex: 1;
    min-width: 0;
  }

  .timeline-title {
    display: block;
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 1.4;
  }

  .timeline-row.git-state .timeline-title {
    color: var(--text-muted);
    font-weight: 400;
  }

  .timeline-title :global(.git-ref-badge) {
    display: inline;
    padding: 1px 6px;
    border: 1px solid var(--border-subtle);
    border-radius: 999px;
    background: var(--bg-hover);
    color: var(--text-primary);
    font: inherit;
    font-weight: 600;
    line-height: inherit;
    vertical-align: baseline;
  }

  .timeline-row.git-state .timeline-title :global(.git-ref-badge) {
    color: var(--text-muted);
    font-weight: inherit;
  }

  .skeleton-title {
    color: var(--text-muted);
  }

  .failed-title {
    color: var(--text-muted);
    font-style: italic;
    font-weight: normal;
  }

  .failed-meta {
    color: var(--text-muted);
  }

  .timeline-meta {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin-top: 3px;
    min-width: 0;
    max-width: 100%;
  }

  .meta-item {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .meta-sha {
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
  }

  .meta-badge {
    display: inline-flex;
    align-items: center;
    flex: 0 0 auto;
    gap: 3px;
    padding: 2px 7px;
    border-radius: 8px;
    background: none;
    border: 1px solid var(--border-subtle);
    color: var(--text-muted);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 600;
    line-height: 1;
  }

  /* Actions container — visible on row hover */
  .timeline-actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex-shrink: 0;
    opacity: 0;
    transition: opacity 0.1s;
  }

  .timeline-row:hover .timeline-actions,
  .timeline-actions.always-visible {
    opacity: 1;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.5;
    }
  }
</style>
