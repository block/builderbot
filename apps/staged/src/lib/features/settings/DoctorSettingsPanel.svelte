<script lang="ts">
  import { onMount } from 'svelte';
  import { RefreshCw, Stethoscope, ClipboardCopy, Check } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import DoctorCheckRow from '../doctor/DoctorCheckRow.svelte';
  import { doctorState, runChecks, formatDebugReport } from '../doctor/doctor.svelte';
  import { refreshProviders } from '../agents/agent.svelte';

  let mounted = true;

  onMount(() => {
    runChecksAndRefresh();
    return () => {
      mounted = false;
    };
  });

  /** Run checks, then refresh the agent selector if still mounted. */
  async function runChecksAndRefresh() {
    await runChecks();
    if (mounted) {
      // Re-discover providers so newly-installed agents are immediately
      // available in the agent selector without requiring an app reload.
      refreshProviders();
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

  let copied = $state(false);

  async function copyDebugInfo() {
    if (!doctorState.report) return;
    const text = formatDebugReport(doctorState.report);
    await navigator.clipboard.writeText(text);
    copied = true;
    setTimeout(() => (copied = false), 2000);
  }
</script>

<div class="doctor-settings-panel">
  <div class="panel-intro">
    <div class="intro-copy">
      <h2>
        <Stethoscope size={16} />
        Doctor
      </h2>
      <p>Verify required tools and agent availability for Staged.</p>
    </div>

    <div class="header-actions">
      {#if doctorState.report && !doctorState.loading}
        <button class="refresh-btn" onclick={copyDebugInfo}>
          {#if copied}
            <Check size={14} />
            Copied
          {:else}
            <ClipboardCopy size={14} />
            Copy details
          {/if}
        </button>
      {/if}

      {#if !doctorState.loading}
        <button class="refresh-btn" onclick={runChecksAndRefresh}>
          <RefreshCw size={14} />
          Re-run
        </button>
      {/if}
    </div>
  </div>

  <div class="panel-body">
    {#if doctorState.loading}
      <div class="loading-state">
        <Spinner size={24} />
        <span>Running checks...</span>
      </div>
    {:else if doctorState.report}
      <div class="section">
        <h3 class="section-label">Tools</h3>
        <div class="checks-list">
          {#each toolChecks as check (check.id)}
            <DoctorCheckRow {check} onFixed={runChecksAndRefresh} />
          {/each}
        </div>
      </div>

      {#if agentChecks.length > 0}
        <div class="section">
          <h3 class="section-label">Agents</h3>
          <div class="checks-list">
            {#each agentChecks as check (check.id)}
              <DoctorCheckRow {check} onFixed={runChecksAndRefresh} />
            {/each}
          </div>
        </div>
      {/if}
    {:else}
      <div class="empty-state">No checks are available yet.</div>
    {/if}
  </div>
</div>

<style>
  .doctor-settings-panel {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border-subtle);
    border-radius: 12px;
    overflow: hidden;
    background: var(--bg-chrome);
  }

  .panel-intro {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 14px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .intro-copy {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }

  .panel-intro h2 {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-md);
    font-weight: 600;
  }

  .panel-intro p {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .panel-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .section-label {
    margin: 0;
    font-size: var(--size-xs);
    font-weight: 600;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .checks-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 280px;
  }

  .loading-state,
  .empty-state {
    color: var(--text-muted);
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 160px;
    font-size: var(--size-sm);
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .refresh-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 12px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    flex-shrink: 0;
    transition:
      color 0.1s,
      border-color 0.1s,
      background-color 0.1s;
  }

  .refresh-btn:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
    background: color-mix(in srgb, var(--bg-hover) 45%, transparent);
  }

  .refresh-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  @media (max-width: 920px) {
    .panel-intro {
      flex-direction: column;
      align-items: flex-start;
    }
  }
</style>
