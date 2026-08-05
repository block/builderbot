<!--
  PrimaryRunActionButton.svelte — the scope's primary run action, driven by an
  ActionRunner.

  Renders as a circular play button (spinner while building, sine wave while
  running, check/alert on completion), or as an endpoint pill with a copy-URL
  button once a serving run action reports its endpoint. Click opens the
  output modal; alt-click stops the running execution.

  Props:
    show               — extra render gate (e.g. hide while a branch is
                         setting up); part of the same if-block as the
                         primary-action check so the slide transition plays
                         for both.
    canResolveEndpoint — false when the endpoint URL can't be rewritten yet
                         (e.g. a remote workspace id is still unknown);
                         suppresses the endpoint pill.
    getEndpointCopyUrl — maps a detected endpoint to the URL the copy button
                         puts on the clipboard (e.g. remote workstation URL
                         rewriting). Defaults to the endpoint itself.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { slide, fade } from 'svelte/transition';
  import Play from '@lucide/svelte/icons/play';
  import Check from '@lucide/svelte/icons/check';
  import CheckCircle from '@lucide/svelte/icons/check-circle';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import StopCircle from '@lucide/svelte/icons/stop-circle';
  import Copy from '@lucide/svelte/icons/copy';
  import Spinner from '../../shared/Spinner.svelte';
  import SineWave from '../../shared/SineWave.svelte';
  import { Button } from '$lib/components/ui/button';
  import type { ActionRunner } from './actionRunner.svelte';
  import { altKey, trackAltKey } from './altKey.svelte';

  interface Props {
    runner: ActionRunner;
    show?: boolean;
    canResolveEndpoint?: boolean;
    getEndpointCopyUrl?: (endpoint: string) => string;
  }

  let {
    runner,
    show = true,
    canResolveEndpoint = true,
    getEndpointCopyUrl = (endpoint) => endpoint,
  }: Props = $props();

  let primaryRunAction = $derived(runner.primaryRunAction);

  // Tracks which endpoint copy buttons are showing the "copied" tick
  let endpointCopied = $state<Record<string, boolean>>({});
  let endpointCopiedTimers: Record<string, ReturnType<typeof setTimeout>> = {};

  onMount(() => trackAltKey());

  onDestroy(() => {
    for (const timer of Object.values(endpointCopiedTimers)) clearTimeout(timer);
  });
</script>

{#if show && primaryRunAction}
  {@const execution = runner.primaryActionExecution}
  {@const isRunning = execution?.status === 'running'}
  {@const isStopping = execution && runner.stoppingExecutions.has(execution.executionId)}
  {@const showStopIcon = altKey.held && isRunning && !isStopping}
  {@const phase = execution ? runner.runPhases.get(execution.executionId) : undefined}
  {@const hasEndpoint = phase?.type === 'running' && !!phase.endpoint && canResolveEndpoint}
  {@const copyUrl =
    hasEndpoint && phase?.type === 'running' && phase.endpoint
      ? getEndpointCopyUrl(phase.endpoint)
      : ''}
  <div
    class="primary-action-container"
    in:slide={{ duration: 300, axis: 'x' }}
    out:slide={{ duration: 300, axis: 'x' }}
  >
    {#if isRunning && hasEndpoint && phase?.type === 'running' && phase.endpoint}
      <!-- Pill-shaped button when running with endpoint -->
      <div class="primary-action-pill">
        <Button
          variant="ghost"
          class={[
            'size-7 rounded-full border-0 bg-transparent text-muted-foreground hover:bg-transparent hover:text-foreground [&_svg]:!size-3.5',
            isStopping && 'opacity-60',
            showStopIcon && 'text-destructive',
          ]}
          title={isStopping
            ? 'Stopping…'
            : showStopIcon
              ? `Stop ${primaryRunAction.name}`
              : `View output for ${primaryRunAction.name}`}
          aria-label={isStopping
            ? 'Stopping'
            : showStopIcon
              ? `Stop ${primaryRunAction.name}`
              : `View output for ${primaryRunAction.name}`}
          onclick={() => {
            if (altKey.held && !isStopping && execution) {
              runner.stopAction(execution.executionId, primaryRunAction.name);
            } else if (execution) {
              runner.showOutput(execution);
            }
          }}
        >
          {#if isStopping}
            <Spinner size={14} class="danger" />
          {:else if showStopIcon}
            <StopCircle size={14} />
          {:else}
            <SineWave size={14} />
          {/if}
        </Button>
        <Button
          variant="ghost"
          class="relative size-7 rounded-none border-0 border-l border-l-[var(--border-muted)] bg-transparent text-muted-foreground hover:bg-[var(--bg-elevated)] hover:text-foreground [&_svg]:!size-3"
          title={`Copy endpoint: ${copyUrl}`}
          aria-label="Copy endpoint"
          onclick={(e) => {
            e.stopPropagation();
            if (phase?.type === 'running' && phase.endpoint && execution && copyUrl) {
              navigator.clipboard.writeText(copyUrl).catch(() => {});
              const id = execution.executionId;
              if (endpointCopiedTimers[id]) clearTimeout(endpointCopiedTimers[id]);
              endpointCopied[id] = true;
              endpointCopiedTimers[id] = setTimeout(() => {
                delete endpointCopied[id];
                delete endpointCopiedTimers[id];
              }, 1500);
            }
          }}
        >
          {#if execution && endpointCopied[execution.executionId]}
            <span
              class="copy-icon-wrapper"
              in:fade={{ duration: 150 }}
              out:fade={{ duration: 150 }}
            >
              <Check size={12} />
            </span>
          {:else}
            <span
              class="copy-icon-wrapper"
              in:fade={{ duration: 150 }}
              out:fade={{ duration: 150 }}
            >
              <Copy size={12} />
            </span>
          {/if}
        </Button>
      </div>
    {:else}
      <!-- Standard circular button -->
      <Button
        variant="ghost"
        class={[
          'size-7 rounded-full border-0 bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] [&_svg]:!size-3.5',
          isRunning && 'bg-[var(--bg-hover)] text-muted-foreground hover:bg-[var(--bg-elevated)]',
          execution?.status === 'completed' &&
            'bg-[var(--bg-hover)] text-[var(--status-added)] hover:bg-[var(--bg-hover)]',
          execution?.status === 'failed' &&
            'bg-[var(--bg-hover)] text-destructive hover:bg-[var(--bg-hover)]',
          isStopping && 'opacity-60 hover:bg-[var(--bg-elevated)]',
          showStopIcon && 'text-destructive',
        ]}
        title={isStopping
          ? 'Stopping…'
          : showStopIcon
            ? `Stop ${primaryRunAction.name}`
            : isRunning
              ? `View output for ${primaryRunAction.name}`
              : execution?.status === 'completed'
                ? `${primaryRunAction.name} completed`
                : execution?.status === 'failed'
                  ? `${primaryRunAction.name} failed`
                  : primaryRunAction.name}
        aria-label={isStopping
          ? 'Stopping'
          : showStopIcon
            ? `Stop ${primaryRunAction.name}`
            : isRunning
              ? `View output for ${primaryRunAction.name}`
              : execution?.status === 'completed'
                ? `${primaryRunAction.name} completed`
                : execution?.status === 'failed'
                  ? `${primaryRunAction.name} failed`
                  : primaryRunAction.name}
        onclick={() => {
          if (isRunning && altKey.held && !isStopping && execution) {
            runner.stopAction(execution.executionId, primaryRunAction.name);
          } else if (isRunning && execution) {
            runner.showOutput(execution);
          } else if (isStopping && execution) {
            runner.showOutput(execution);
          } else {
            runner.runAction(primaryRunAction);
          }
        }}
      >
        {#if isStopping}
          <Spinner size={14} class="danger" />
        {:else if showStopIcon}
          <StopCircle size={14} />
        {:else if isRunning && phase?.type === 'building'}
          <Spinner size={14} />
        {:else if isRunning}
          <SineWave size={14} />
        {:else if execution?.status === 'completed'}
          <CheckCircle size={14} />
        {:else if execution?.status === 'failed'}
          <AlertCircle size={14} />
        {:else}
          <Play size={14} />
        {/if}
      </Button>
    {/if}
  </div>
{/if}

<style>
  /* Primary action button — circular icon-only */
  .primary-action-container {
    display: flex;
    align-items: center;
    overflow: hidden;
  }

  /* Primary action pill (endpoint running state) */
  .primary-action-pill {
    display: flex;
    align-items: center;
    height: 28px;
    background: var(--bg-hover);
    border-radius: 999px;
    overflow: hidden;
  }

  .copy-icon-wrapper {
    position: absolute;
    display: flex;
    align-items: center;
    justify-content: center;
  }
</style>
