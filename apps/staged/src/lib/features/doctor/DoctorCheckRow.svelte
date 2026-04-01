<!--
  DoctorCheckRow.svelte — A single row in the doctor report.

  Shows a status icon (✓ / ⚠ / ✗), the check label, a message,
  and optional action buttons: an external-link icon to open an
  install page, or a "Fix" button that runs a shell command.
-->
<script lang="ts">
  import { CheckCircle, AlertTriangle, XCircle, ExternalLink, Wrench } from 'lucide-svelte';
  import { openUrl, runDoctorFix } from '../../api/commands';
  import type { DoctorCheck } from '../../api/commands';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';

  let {
    check,
    onFixed,
  }: {
    check: DoctorCheck;
    onFixed?: () => void;
  } = $props();

  let fixing = $state(false);
  let fixError = $state<string | null>(null);
  let showFixDialog = $state(false);

  function promptFix() {
    if (!check.fixType) return;
    fixError = null;
    fixing = false;
    showFixDialog = true;
  }

  async function confirmFix() {
    if (!check.fixType) return;
    fixing = true;
    fixError = null;
    try {
      await runDoctorFix(check.id, check.fixType);
      showFixDialog = false;
      onFixed?.();
    } catch (e) {
      fixError = String(e);
    } finally {
      fixing = false;
    }
  }

  function cancelFix() {
    if (fixing) return;
    showFixDialog = false;
  }
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
    {#if check.path}
      <span class="check-path">{check.path}</span>
    {/if}
    {#if check.bridgePath}
      <span class="check-path">{check.bridgePath}</span>
    {/if}
  </div>

  {#if check.fixType && check.fixCommand && check.status !== 'pass'}
    <button class="fix-btn" onclick={promptFix}>
      <Wrench size={14} />
      Fix
    </button>
  {/if}

  {#if check.fixUrl && check.status !== 'pass'}
    <button class="install-btn" onclick={() => openUrl(check.fixUrl!)}>
      <ExternalLink size={14} />
    </button>
  {/if}
</div>

{#if showFixDialog}
  <ConfirmDialog
    title="Run fix command?"
    message={check.fixCommand!}
    confirmLabel={fixing ? 'Running' : fixError ? 'Retry' : 'Run'}
    cancelLabel="Cancel"
    confirmDisabled={fixing}
    cancelDisabled={fixing}
    error={fixError}
    onConfirm={confirmFix}
    onCancel={cancelFix}
  />
{/if}

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
    overflow-wrap: break-word;
    word-wrap: break-word;
  }

  .check-path {
    font-size: 10px;
    color: var(--text-faint, rgba(255, 255, 255, 0.35));
    font-family: monospace;
    overflow-wrap: break-word;
    word-wrap: break-word;
  }

  .fix-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    padding: 4px 8px;
    background: none;
    border: 1px solid var(--border-primary, rgba(255, 255, 255, 0.1));
    color: var(--text-muted);
    cursor: pointer;
    border-radius: 4px;
    font-size: var(--size-xs);
    transition:
      color 0.1s,
      background 0.1s,
      border-color 0.1s;
  }

  .fix-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover, rgba(255, 255, 255, 0.06));
    border-color: var(--border-hover, rgba(255, 255, 255, 0.2));
  }

  .install-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    padding: 4px;
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    border-radius: 4px;
    transition:
      color 0.1s,
      background 0.1s;
  }

  .install-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover, rgba(255, 255, 255, 0.06));
  }
</style>
