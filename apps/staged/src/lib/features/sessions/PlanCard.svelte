<!--
  PlanCard.svelte — the latest ACP plan, pinned above the chat transcript.

  Renders a collapsible card summarizing the agent's current plan. The header
  shows a caret, an overall status icon mirroring tool-call semantics, the
  "Plan" title, and a muted completed/total progress summary. The body lists
  each entry with a per-status icon.

  Props:
    entries         — the plan entries (never empty; callers gate on latestPlan)
    defaultExpanded — initial expansion; user toggles are respected afterwards
-->
<script lang="ts">
  import { slide } from 'svelte/transition';
  import CircleAlert from '@lucide/svelte/icons/circle-alert';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Spinner from '../../shared/Spinner.svelte';
  import type { PlanEntry } from './acpTranscript';

  interface Props {
    entries: PlanEntry[];
    defaultExpanded: boolean;
  }

  let { entries, defaultExpanded }: Props = $props();

  // Initialized once (capturing the initial value is intentional); new plan
  // updates streaming in never override the user's explicit toggle.
  // svelte-ignore state_referenced_locally
  let expanded = $state(defaultExpanded);

  let completedCount = $derived(entries.filter((entry) => entry.status === 'completed').length);
  let overallStatus = $derived.by((): PlanEntry['status'] | null => {
    if (entries.some((entry) => entry.status === 'failed')) return 'failed';
    if (entries.some((entry) => entry.status === 'in_progress')) return 'in_progress';
    if (entries.length > 0 && entries.every((entry) => entry.status === 'completed'))
      return 'completed';
    return null;
  });
</script>

<div class="plan-card">
  <button
    type="button"
    class="plan-header"
    aria-expanded={expanded}
    onclick={() => (expanded = !expanded)}
  >
    <span class="plan-caret" class:plan-caret-expanded={expanded}>
      <ChevronRight size={12} />
    </span>
    {#if overallStatus === 'in_progress'}
      <span class="plan-status-icon"><Spinner size={11} /></span>
    {:else if overallStatus === 'failed'}
      <span class="plan-status-icon status-danger"><CircleAlert size={11} /></span>
    {:else if overallStatus === 'completed'}
      <span class="plan-status-icon status-success"><CircleCheck size={11} /></span>
    {/if}
    <span class="plan-title">Plan</span>
    <span class="plan-progress">{completedCount}/{entries.length}</span>
  </button>
  {#if expanded}
    <div class="plan-entries" transition:slide={{ duration: 150 }}>
      {#each entries as entry}
        <div class="plan-entry">
          <span
            class="plan-entry-icon"
            class:status-success={entry.status === 'completed'}
            class:status-danger={entry.status === 'failed'}
          >
            {#if entry.status === 'in_progress'}
              <Spinner size={11} />
            {:else if entry.status === 'completed'}
              <CircleCheck size={11} />
            {:else if entry.status === 'failed'}
              <CircleAlert size={11} />
            {/if}
          </span>
          <span class="plan-entry-text">{entry.content}</span>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .plan-card {
    flex-shrink: 0;
    border-bottom: 1px solid var(--border-subtle);
    background: var(--bg-secondary);
    padding: 6px 16px;
    font-size: var(--size-xs);
  }

  .plan-header {
    display: flex;
    align-items: center;
    gap: 4px;
    width: 100%;
    border: 0;
    background: transparent;
    color: var(--text-muted);
    padding: 2px 0;
    text-align: left;
    font: inherit;
    cursor: pointer;
  }

  .plan-header:hover .plan-title {
    text-decoration: underline;
  }

  .plan-caret {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 8px;
    height: 12px;
    color: var(--text-faint);
  }

  .plan-caret :global(svg) {
    transition: transform 0.15s ease;
  }

  .plan-caret-expanded :global(svg) {
    transform: rotate(90deg);
  }

  .plan-title {
    flex-shrink: 0;
    font-weight: 500;
    line-height: 1;
    transform: translateY(-0.5px);
  }

  .plan-progress {
    color: var(--text-faint);
  }

  .plan-status-icon,
  .plan-entry-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 12px;
    height: 12px;
    flex-shrink: 0;
    color: var(--text-faint);
  }

  .status-success {
    color: var(--ui-success, var(--ui-accent));
  }

  .status-danger {
    color: var(--ui-danger);
  }

  .plan-entries {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 4px;
    padding: 2px 0 4px;
  }

  .plan-entry {
    display: flex;
    gap: 6px;
    align-items: flex-start;
    color: var(--text-muted);
    line-height: 1.35;
  }

  .plan-entry-icon {
    /* Reserve the slot even for pending entries so text stays aligned. */
    margin-top: 2px;
  }

  .plan-entry-text {
    min-width: 0;
  }
</style>
