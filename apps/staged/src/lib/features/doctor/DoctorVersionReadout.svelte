<!--
  DoctorVersionReadout.svelte — one binary behind an agent check in a Doctor
  row: which one it is (main agent / ACP bridge), where it came from ("Managed
  by Staged" or its path), the version installed, and the update badge when a
  newer release of the same package is known.

  Versions are shown whether or not an update is available; a version that has
  not been probed yet reads "checking", one the probe could not read "unknown".
  What the readout describes is the install new launches use, not the code a
  running agent session has already loaded.
-->
<script lang="ts">
  import ArrowUpCircle from '@lucide/svelte/icons/arrow-up-circle';
  import type { AgentVersionInfo } from '../../api/commands';
  import { describeVersionReadout, type ReadoutKind } from './versionReadout';

  let {
    kind,
    path,
    info,
    loading,
  }: {
    kind: ReadoutKind;
    /** The resolved executable, shown unless the install is Staged-managed. */
    path: string;
    info: AgentVersionInfo | null;
    /** The freshness pass is in flight, so a missing version may still arrive. */
    loading: boolean;
  } = $props();

  const view = $derived(describeVersionReadout({ kind, path, info, loading }));
</script>

<div class="readout">
  <span class="readout-line">
    <span class="readout-label">{view.label}</span>
    <span class="readout-location">{view.location}</span>
    {#if view.version !== null}
      <span class="readout-version">{view.version}</span>
      {#if view.upToDate}
        <span class="readout-note">up to date</span>
      {/if}
    {:else if view.versionState === 'checking'}
      <span class="readout-note">checking version…</span>
    {:else}
      <span class="readout-note">version unknown</span>
    {/if}
  </span>
  {#if view.badge}
    <span class="update-badge" class:info-only={view.badge.infoOnly}>
      <ArrowUpCircle size={11} />
      {view.badge.text}
    </span>
  {/if}
</div>

<style>
  .readout {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .readout-line {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    column-gap: 6px;
    font-size: 10px;
    font-family: monospace;
    color: var(--text-faint, rgba(255, 255, 255, 0.35));
    overflow-wrap: break-word;
    word-wrap: break-word;
    min-width: 0;
  }

  .readout-label {
    font-family: inherit;
    color: var(--text-muted);
  }

  .readout-location {
    overflow-wrap: anywhere;
  }

  .readout-version {
    color: var(--text-muted);
  }

  .readout-note {
    font-style: italic;
  }

  .update-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    align-self: flex-start;
    margin-top: 2px;
    font-size: 10px;
    color: var(--color-warning, #d29922);
  }

  /* When there's no runnable command, the badge is informational only. */
  .update-badge.info-only {
    color: var(--text-muted);
  }
</style>
