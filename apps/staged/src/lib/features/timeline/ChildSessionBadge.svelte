<!--
  ChildSessionBadge – pill for one in-flight repo session spawned by a project
  session, labelled with the child's own title.

  Shaped like a hashtag badge and coloured the way the branch timeline colours
  that kind of session (note / commit / review), with a spinner where the
  hashtag badge puts its type icon — or a clock while the child is still
  waiting on the branch queue, as the branch timeline's queued rows do.

  A running child's badge is a button that opens the child's chat, and stops
  the click so the parent row's own click action doesn't fire too. Queued
  children have no chat yet (the branch card hides their chat button), so the
  badge is a plain span.
-->
<script lang="ts">
  import Clock from '@lucide/svelte/icons/clock';
  import Spinner from '../../shared/Spinner.svelte';
  import type { SessionRowKind } from './sessionRowTitle';

  interface Props {
    sessionType: SessionRowKind;
    status: 'queued' | 'running';
    /** The clipped label. */
    label: string;
    /** The whole title, shown in the tooltip. */
    title: string;
    onclick?: () => void;
  }

  let { sessionType, status, label, title, onclick }: Props = $props();

  const KIND_LABEL: Record<SessionRowKind, string> = {
    commit: 'Commit',
    note: 'Note',
    review: 'Review',
  };

  let tooltip = $derived(
    `${KIND_LABEL[sessionType]}${status === 'queued' ? ' (queued)' : ''}: ${title}`
  );
  let clickable = $derived(status === 'running' && !!onclick);

  function handleClick(e: MouseEvent) {
    e.stopPropagation();
    onclick?.();
  }
</script>

{#snippet body()}
  {#if status === 'queued'}
    <Clock size={12} />
  {:else}
    <Spinner size={12} />
  {/if}
  <span class="child-session-badge-label">{label}</span>
{/snippet}

{#if clickable}
  <button
    type="button"
    class="child-session-badge type-{sessionType} stable-raster"
    title={tooltip}
    aria-label={`Open chat — ${tooltip}`}
    onclick={handleClick}
  >
    {@render body()}
  </button>
{:else}
  <span class="child-session-badge type-{sessionType} queued stable-raster" title={tooltip}>
    {@render body()}
  </span>
{/if}

<style>
  /* Matches .hashtag-badge in app.css, as an inline-flex so the spinner and
     label share a baseline the way that badge's icon and text do. */
  .child-session-badge {
    display: inline-flex;
    align-items: center;
    flex: 0 1 auto;
    min-width: 0;
    max-width: 100%;
    gap: 3px;
    padding: 3px 6px;
    border: 0;
    border-radius: 4px;
    font: inherit;
    font-size: 0.85em;
    font-weight: 500;
    line-height: 1.35;
    white-space: nowrap;
    text-align: left;
    cursor: default;
  }

  button.child-session-badge {
    cursor: pointer;
  }

  button.child-session-badge:hover {
    filter: brightness(1.1);
  }

  button.child-session-badge:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: 1px;
  }

  .child-session-badge-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* The branch timeline's per-kind colours, the same variables its icons and
     the hashtag badges use. */
  .child-session-badge.type-commit {
    background: var(--commit-bg);
    color: var(--commit-color);
  }

  .child-session-badge.type-note {
    background: var(--note-bg);
    color: var(--note-color);
  }

  .child-session-badge.type-review {
    background: var(--review-bg);
    color: var(--review-color);
  }

  /* Waiting on the branch queue: dimmed, like a queued timeline row. */
  .child-session-badge.queued {
    opacity: 0.7;
  }
</style>
