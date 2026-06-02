<!--
  DoctorCheckRow.svelte — A single row in the doctor report.

  Shows a status icon (✓ / ⚠ / ✗), the check label, a message,
  and optional action buttons: an external-link icon to open an
  install page, or a "Fix" button that runs a shell command.
-->
<script lang="ts">
  import CheckCircle from '@lucide/svelte/icons/check-circle';
  import AlertTriangle from '@lucide/svelte/icons/alert-triangle';
  import XCircle from '@lucide/svelte/icons/x-circle';
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import Wrench from '@lucide/svelte/icons/wrench';
  import { openUrl, runDoctorFix } from '../../api/commands';
  import type { DoctorCheck } from '../../api/commands';
  import { Button } from '$lib/components/ui/button';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';

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
    <Button variant="outline" size="sm" onclick={promptFix}>
      <Wrench size={14} />
      Fix
    </Button>
  {/if}

  {#if check.fixUrl && check.status !== 'pass'}
    <Button variant="ghost" size="icon" onclick={() => openUrl(check.fixUrl!)}>
      <ExternalLink size={14} />
    </Button>
  {/if}
</div>

<AlertDialog.Root bind:open={showFixDialog}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Run fix command?</AlertDialog.Title>
      <AlertDialog.Description class="max-h-[42vh] overflow-auto whitespace-pre-line">
        {check.fixCommand}
      </AlertDialog.Description>
    </AlertDialog.Header>
    {#if fixError}
      <p class="text-destructive text-sm">{fixError}</p>
    {/if}
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={fixing} onclick={cancelFix}>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="outline"
        disabled={fixing}
        onclick={(e) => {
          e.preventDefault();
          confirmFix();
        }}
      >
        {fixing ? 'Running' : fixError ? 'Retry' : 'Run'}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

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
</style>
