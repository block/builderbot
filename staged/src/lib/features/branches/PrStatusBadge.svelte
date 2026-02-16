<!--
  PrStatusBadge.svelte - Display PR check status with icon and color

  Shows the status of PR checks with appropriate visual indicators:
  - Pending: Yellow/orange spinner
  - Success: Green checkmark
  - Failure: Red X
  - Error: Red alert icon

  Also displays check counts when available (e.g., "3/5 passing")
-->
<script lang="ts">
  import { CheckCircle2, XCircle, AlertCircle, Loader2 } from 'lucide-svelte';
  import type { PrStatusState } from '../../types';

  interface Props {
    state: PrStatusState;
    checksTotal?: number | null;
    checksPassed?: number | null;
    checksFailed?: number | null;
    checksPending?: number | null;
  }

  let { state, checksTotal, checksPassed, checksFailed, checksPending }: Props = $props();

  // Determine the display text based on check counts
  let statusText = $derived.by(() => {
    if (state === 'pending') {
      if (checksPending && checksTotal) {
        return `${checksPending} pending`;
      }
      return 'Checks pending';
    } else if (state === 'success') {
      if (checksTotal) {
        return `${checksTotal} passing`;
      }
      return 'Checks passed';
    } else if (state === 'failure') {
      if (checksFailed && checksTotal) {
        return `${checksFailed}/${checksTotal} failed`;
      }
      return 'Checks failed';
    } else if (state === 'error') {
      return 'Check error';
    }
    return 'Unknown';
  });

  // Determine the icon component
  let Icon = $derived.by(() => {
    switch (state) {
      case 'pending':
        return Loader2;
      case 'success':
        return CheckCircle2;
      case 'failure':
        return XCircle;
      case 'error':
        return AlertCircle;
      default:
        return AlertCircle;
    }
  });
</script>

<div
  class="pr-status-badge"
  class:pending={state === 'pending'}
  class:success={state === 'success'}
  class:failure={state === 'failure'}
  class:error={state === 'error'}
>
  <Icon size={12} class={state === 'pending' ? 'spinner' : ''} />
  <span class="status-text">{statusText}</span>
</div>

<style>
  .pr-status-badge {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 3px 10px;
    border-radius: 12px;
    font-size: var(--size-xs);
    font-weight: 500;
    white-space: nowrap;
  }

  .pr-status-badge.pending {
    background-color: rgba(210, 153, 34, 0.1);
    color: rgb(210, 153, 34);
  }

  .pr-status-badge.success {
    background-color: rgba(63, 185, 80, 0.1);
    color: rgb(63, 185, 80);
  }

  .pr-status-badge.failure {
    background-color: rgba(248, 81, 73, 0.1);
    color: var(--ui-danger);
  }

  .pr-status-badge.error {
    background-color: rgba(248, 81, 73, 0.1);
    color: var(--ui-danger);
  }

  .pr-status-badge :global(svg) {
    flex-shrink: 0;
  }

  .pr-status-badge :global(.spinner) {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .status-text {
    line-height: 1;
  }
</style>
