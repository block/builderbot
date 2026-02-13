<!--
  DoctorCheckRow.svelte — A single row in the doctor report.

  Shows a status icon (✓ / ⚠ / ✗), the check label, a message,
  and an optional "Install" link that opens the relevant page in
  the user's browser.
-->
<script lang="ts">
  import { CheckCircle, AlertTriangle, XCircle, ExternalLink } from 'lucide-svelte';
  import { openUrl } from '../../commands';
  import type { DoctorCheck } from '../../commands';

  let {
    check,
  }: {
    check: DoctorCheck;
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

  {#if check.fixUrl && check.status !== 'pass'}
    <button class="install-link" onclick={() => openUrl(check.fixUrl!)}>
      <ExternalLink size={12} />
      Install
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

  .install-link {
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

  .install-link:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
  }
</style>
