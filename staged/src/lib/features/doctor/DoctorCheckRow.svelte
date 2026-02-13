<!--
  DoctorCheckRow.svelte — A single row in the doctor report.

  Shows a status icon (✓ / ⚠ / ✗), the check label, a message,
  and an optional "Fix" button when a fix_command is available.
-->
<script lang="ts">
  import { CheckCircle, AlertTriangle, XCircle, Wrench } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import type { DoctorCheck } from '../../commands';

  let {
    check,
    fixing = false,
    onfix,
  }: {
    check: DoctorCheck;
    fixing?: boolean;
    onfix?: (id: string) => void;
  } = $props();
</script>

<div
  class="check-row"
  class:pass={check.status === 'pass'}
  class:warn={check.status === 'warn'}
  class:fail={check.status === 'fail'}
>
  <div class="status-icon">
    {#if check.status === 'pass'}
      <CheckCircle size={16} />
    {:else if check.status === 'warn'}
      <AlertTriangle size={16} />
    {:else}
      <XCircle size={16} />
    {/if}
  </div>

  <div class="check-info">
    <span class="check-label">{check.label}</span>
    <span class="check-message">{check.message}</span>
  </div>

  {#if check.fixCommand && check.status !== 'pass'}
    <button class="fix-btn" disabled={fixing} onclick={() => onfix?.(check.id)}>
      {#if fixing}
        <Spinner size={12} />
        Fixing…
      {:else}
        <Wrench size={12} />
        Fix
      {/if}
    </button>
  {/if}
</div>

<style>
  .check-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    background: var(--bg-primary);
    border-radius: 8px;
  }

  .status-icon {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .check-row.pass .status-icon {
    color: var(--color-success, #3fb950);
  }

  .check-row.warn .status-icon {
    color: var(--color-warning, #d29922);
  }

  .check-row.fail .status-icon {
    color: var(--color-danger, #f85149);
  }

  .check-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .check-label {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .check-message {
    font-size: var(--size-xs);
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .fix-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-family: inherit;
    white-space: nowrap;
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .fix-btn:not(:disabled):hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
  }

  .fix-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
