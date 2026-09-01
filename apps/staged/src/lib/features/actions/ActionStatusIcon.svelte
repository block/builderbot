<!--
  ActionStatusIcon.svelte — the icon inside a button that represents one
  action's live state, shared by the pinned-action buttons and the
  running-action pills.

  Rungs, most specific first: stopping → danger spinner; offering to stop →
  stop circle; serving → sine wave; otherwise running → spinner;
  completed/failed → check/alert; nothing running → the `idle` snippet.

  `serving` is a boolean the caller computes rather than something derived
  here: the surfaces disagree on what a running run action with no run phase
  yet looks like (sine wave in a header button, spinner in a pill), so the
  expression stays at the call site.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import CheckCircle from '@lucide/svelte/icons/check-circle';
  import StopCircle from '@lucide/svelte/icons/stop-circle';
  import Spinner from '../../shared/Spinner.svelte';
  import SineWave from '../../shared/SineWave.svelte';
  import type { ActionStatus } from './actions';

  interface Props {
    /** A stop has been requested and the process hasn't exited yet. */
    stopping: boolean;
    /** The button is currently offering to stop the action (alt held). */
    showStop: boolean;
    /** The action is past its build phase and serving. */
    serving: boolean;
    /** The execution is live. */
    running: boolean;
    /** The execution's status, when there is an execution at all. */
    status?: ActionStatus;
    size: number;
    /** Rendered when nothing is running and there's no outcome to report. */
    idle?: Snippet;
  }

  let { stopping, showStop, serving, running, status, size, idle }: Props = $props();
</script>

{#if stopping}
  <Spinner {size} class="danger" />
{:else if showStop}
  <StopCircle {size} />
{:else if serving}
  <SineWave {size} />
{:else if running}
  <Spinner {size} />
{:else if status === 'completed'}
  <CheckCircle {size} />
{:else if status === 'failed'}
  <AlertCircle {size} />
{:else}
  {@render idle?.()}
{/if}
