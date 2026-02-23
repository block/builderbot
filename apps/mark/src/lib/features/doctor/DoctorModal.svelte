<!--
  DoctorModal.svelte — Health Check modal.

  Opened from the Settings "Health Check" section.
  Runs all checks on mount and displays results with optional fix buttons.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { X, Stethoscope, RefreshCw } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import DoctorCheckRow from './DoctorCheckRow.svelte';
  import { doctorState, runChecks } from './doctor.svelte';

  let { onClose }: { onClose: () => void } = $props();

  onMount(() => {
    runChecks();
  });

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onClose();
    }
  }

  /** Tool checks (non-agent). */
  const toolChecks = $derived(
    doctorState.report?.checks.filter((c) => !c.id.startsWith('ai-agent-')) ?? []
  );

  /** Agent checks. */
  const agentChecks = $derived(
    doctorState.report?.checks.filter((c) => c.id.startsWith('ai-agent-')) ?? []
  );
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="modal-backdrop" role="dialog" aria-modal="true" tabindex="-1" onkeydown={handleKeydown}>
  <div class="modal" role="presentation" onclick={(e) => e.stopPropagation()}>
    <button class="close-btn" onclick={onClose} aria-label="Close">
      <X size={16} />
    </button>

    <div class="modal-header">
      <div class="header-icon">
        <Stethoscope size={24} />
      </div>
      <h2>Health Check</h2>
      <p class="subtitle">
        {#if doctorState.loading}
          Checking your environment…
        {:else if doctorState.report}
          Verifying required tools and configuration
        {:else}
          Verifying required tools and configuration
        {/if}
      </p>
    </div>

    <div class="modal-body">
      {#if doctorState.loading}
        <div class="loading-state">
          <Spinner size={24} />
          <span>Running checks…</span>
        </div>
      {:else if doctorState.report}
        <div class="section">
          <h3 class="section-label">Tools</h3>
          <div class="checks-list">
            {#each toolChecks as check (check.id)}
              <DoctorCheckRow {check} onFixed={runChecks} />
            {/each}
          </div>
        </div>

        {#if agentChecks.length > 0}
          <div class="section">
            <h3 class="section-label">Agents</h3>
            <div class="checks-list">
              {#each agentChecks as check (check.id)}
                <DoctorCheckRow {check} onFixed={runChecks} />
              {/each}
            </div>
          </div>
        {/if}
      {/if}
    </div>

    <div class="modal-footer">
      <button class="refresh-btn" disabled={doctorState.loading} onclick={runChecks}>
        {#if doctorState.loading}
          <Spinner size={14} />
        {:else}
          <RefreshCw size={14} />
        {/if}
        Re-run
      </button>
      <button class="dismiss-btn" onclick={onClose}>Done</button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    position: relative;
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 460px;
    max-width: 90vw;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .close-btn {
    position: absolute;
    top: 12px;
    right: 12px;
    background: none;
    border: none;
    color: var(--text-faint);
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: color 0.1s;
  }

  .close-btn:hover {
    color: var(--text-primary);
  }

  .modal-header {
    padding: 24px 24px 16px;
    text-align: center;
  }

  .header-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 48px;
    height: 48px;
    margin: 0 auto 12px;
    background: var(--bg-primary);
    border-radius: 12px;
    color: var(--text-muted);
  }

  .modal-header h2 {
    margin: 0 0 6px 0;
    font-size: calc(var(--size-base) + 2px);
    font-weight: 600;
    color: var(--text-primary);
  }

  .subtitle {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
    line-height: 1.4;
    word-wrap: break-word;
    overflow-wrap: break-word;
  }

  .modal-body {
    padding: 0 24px;
  }

  .loading-state {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 32px 0;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .section {
    margin-bottom: 12px;
  }

  .section:last-child {
    margin-bottom: 0;
  }

  .section-label {
    margin: 0 0 6px 0;
    font-size: var(--size-xs);
    font-weight: 600;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .checks-list {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 6px;
  }

  .modal-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 24px;
    margin-top: 8px;
  }

  .refresh-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    font-family: inherit;
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .refresh-btn:not(:disabled):hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
  }

  .refresh-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .dismiss-btn {
    padding: 8px 16px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    font-family: inherit;
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .dismiss-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
  }
</style>
