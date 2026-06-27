<script lang="ts">
  import { onMount } from 'svelte';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import Stethoscope from '@lucide/svelte/icons/stethoscope';
  import ClipboardCopy from '@lucide/svelte/icons/clipboard-copy';
  import Check from '@lucide/svelte/icons/check';
  import ArrowUpCircle from '@lucide/svelte/icons/arrow-up-circle';
  import Spinner from '../../shared/Spinner.svelte';
  import DoctorCheckRow from '../doctor/DoctorCheckRow.svelte';
  import {
    doctorState,
    runChecks,
    updateAll,
    hasActionableUpdate,
    formatDebugReport,
  } from '../doctor/doctor.svelte';
  import { refreshProviders } from '../agents/agent.svelte';
  import { Button } from '$lib/components/ui/button';

  let mounted = true;

  onMount(() => {
    runChecksAndRefresh();
    return () => {
      mounted = false;
    };
  });

  /** Run checks, then refresh the agent selector if still mounted. */
  async function runChecksAndRefresh(options: { forceProviderRefresh?: boolean } = {}) {
    await runChecks();
    if (mounted) {
      // Re-discover providers so newly-installed agents are immediately
      // available in the agent selector without requiring an app reload.
      refreshProviders({ force: options.forceProviderRefresh });
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

  /** True when any check has an actionable update across its readouts. */
  const anyUpdatable = $derived(doctorState.report?.checks.some(hasActionableUpdate) ?? false);

  async function runUpdateAll() {
    if (doctorState.updatingAll) return;
    // updateAll owns `doctorState.updatingAll` for its duration, which disables
    // every per-row Update/Fix button so no individual update can race the batch.
    await updateAll();
    // One full re-run after the batch: re-derives each check's status/message
    // (so updated tools drop their stale warnings), chains a freshness pass to
    // clear the badges, and re-discovers providers.
    if (mounted) await runChecksAndRefresh();
  }

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
        <Button variant="outline" size="sm" onclick={copyDebugInfo}>
          {#if copied}
            <Check size={14} />
            Copied
          {:else}
            <ClipboardCopy size={14} />
            Copy details
          {/if}
        </Button>
      {/if}

      {#if anyUpdatable && !doctorState.loading}
        <Button
          variant="outline"
          size="sm"
          disabled={doctorState.updatingAll}
          onclick={runUpdateAll}
        >
          <ArrowUpCircle size={14} />
          {doctorState.updatingAll ? 'Updating all' : 'Update all'}
        </Button>
      {/if}

      {#if !doctorState.loading}
        <Button
          variant="outline"
          size="sm"
          disabled={doctorState.updatingAll}
          onclick={() => runChecksAndRefresh({ forceProviderRefresh: true })}
        >
          <RefreshCw size={14} />
          Re-run
        </Button>
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
              <DoctorCheckRow
                {check}
                agentId={check.id.replace('ai-agent-', '')}
                onFixed={runChecksAndRefresh}
              />
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
    max-width: 560px;
    margin: 0 auto;
    width: 100%;
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

  @media (max-width: 920px) {
    .panel-intro {
      flex-direction: column;
      align-items: flex-start;
    }
  }
</style>
