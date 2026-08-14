<!--
  PinnedActionButton.svelte — one pinned action's button in a card header,
  driven by an ActionRunner.

  Renders as a circular button showing the action's icon (spinner while
  building, sine wave while a run action serves, check/alert on completion), or
  as an endpoint pill with a copy-URL button once a serving run action reports
  its endpoint. Click opens the output modal; alt-click stops the running
  execution. A header renders one of these per pinned action, each tracking its
  own execution — pinned non-run actions simply never reach the serving or
  endpoint states.

  Props:
    action             — the pinned action this button runs.
    show               — extra render gate (e.g. hide while a branch is
                         setting up); wraps the whole button so the slide
                         transition plays for it.
    canResolveEndpoint — false when the endpoint URL can't be rewritten yet
                         (e.g. a remote workspace id is still unknown);
                         suppresses the endpoint pill.
    getEndpointCopyUrl — maps a detected endpoint to the URL the copy button
                         puts on the clipboard (e.g. remote workstation URL
                         rewriting). Defaults to the endpoint itself.
    variant            — surface theme: 'default' is the branch card's
                         elevated neutral button; 'outline' is a clear
                         background outlined with the host card's theme,
                         reading the --card-border-hover / --card-bg-strong
                         custom properties the repo card sets from its badge
                         hue.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { slide, fade } from 'svelte/transition';
  import Check from '@lucide/svelte/icons/check';
  import CheckCircle from '@lucide/svelte/icons/check-circle';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import StopCircle from '@lucide/svelte/icons/stop-circle';
  import Copy from '@lucide/svelte/icons/copy';
  import Spinner from '../../shared/Spinner.svelte';
  import SineWave from '../../shared/SineWave.svelte';
  import { Button } from '$lib/components/ui/button';
  import type { ProjectAction } from '../../api/commands';
  import type { ActionRunner } from './actionRunner.svelte';
  import { altKey, trackAltKey } from './altKey.svelte';
  import ActionIcon from './ActionIcon.svelte';

  interface Props {
    runner: ActionRunner;
    action: ProjectAction;
    show?: boolean;
    canResolveEndpoint?: boolean;
    getEndpointCopyUrl?: (endpoint: string) => string;
    variant?: 'default' | 'outline';
  }

  let {
    runner,
    action,
    show = true,
    canResolveEndpoint = true,
    getEndpointCopyUrl = (endpoint) => endpoint,
    variant = 'default',
  }: Props = $props();

  let outline = $derived(variant === 'outline');

  // Tracks which endpoint copy buttons are showing the "copied" tick
  let endpointCopied = $state<Record<string, boolean>>({});
  let endpointCopiedTimers: Record<string, ReturnType<typeof setTimeout>> = {};

  onMount(() => trackAltKey());

  onDestroy(() => {
    for (const timer of Object.values(endpointCopiedTimers)) clearTimeout(timer);
  });
</script>

{#if show}
  {@const execution = runner.executionFor(action.id)}
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
    class="pinned-action-container"
    in:slide={{ duration: 300, axis: 'x' }}
    out:slide={{ duration: 300, axis: 'x' }}
  >
    {#if isRunning && hasEndpoint && phase?.type === 'running' && phase.endpoint}
      <!-- Pill-shaped button when running with endpoint -->
      <div class="pinned-action-pill" class:outline>
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
              ? `Stop ${action.name}`
              : `View output for ${action.name}`}
          aria-label={isStopping
            ? 'Stopping'
            : showStopIcon
              ? `Stop ${action.name}`
              : `View output for ${action.name}`}
          onclick={() => {
            if (altKey.held && !isStopping && execution) {
              runner.stopAction(execution.executionId, action.name);
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
          class={[
            'relative size-7 rounded-none border-0 border-l bg-transparent text-muted-foreground hover:text-foreground [&_svg]:!size-3',
            outline
              ? 'border-l-[var(--card-border-hover)] hover:bg-[var(--card-bg-strong)]'
              : 'border-l-[var(--border-muted)] hover:bg-[var(--bg-elevated)]',
          ]}
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
          'size-7 rounded-full [&_svg]:!size-3.5',
          outline
            ? 'border border-[var(--card-border-hover)] bg-transparent hover:bg-[var(--card-bg-strong)]'
            : 'border-0 bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)]',
          isRunning &&
            (outline
              ? 'text-muted-foreground'
              : 'bg-[var(--bg-hover)] text-muted-foreground hover:bg-[var(--bg-elevated)]'),
          execution?.status === 'completed' &&
            (outline
              ? 'border-[var(--status-added)] text-[var(--status-added)]'
              : 'bg-[var(--bg-hover)] text-[var(--status-added)] hover:bg-[var(--bg-hover)]'),
          execution?.status === 'failed' &&
            (outline
              ? 'border-destructive text-destructive'
              : 'bg-[var(--bg-hover)] text-destructive hover:bg-[var(--bg-hover)]'),
          isStopping &&
            (outline
              ? 'opacity-60 hover:bg-transparent'
              : 'opacity-60 hover:bg-[var(--bg-elevated)]'),
          showStopIcon && (outline ? 'border-destructive text-destructive' : 'text-destructive'),
        ]}
        title={isStopping
          ? 'Stopping…'
          : showStopIcon
            ? `Stop ${action.name}`
            : isRunning
              ? `View output for ${action.name}`
              : execution?.status === 'completed'
                ? `${action.name} completed`
                : execution?.status === 'failed'
                  ? `${action.name} failed`
                  : action.name}
        aria-label={isStopping
          ? 'Stopping'
          : showStopIcon
            ? `Stop ${action.name}`
            : isRunning
              ? `View output for ${action.name}`
              : execution?.status === 'completed'
                ? `${action.name} completed`
                : execution?.status === 'failed'
                  ? `${action.name} failed`
                  : action.name}
        onclick={() => {
          if (isRunning && altKey.held && !isStopping && execution) {
            runner.stopAction(execution.executionId, action.name);
          } else if (isRunning && execution) {
            runner.showOutput(execution);
          } else if (isStopping && execution) {
            runner.showOutput(execution);
          } else {
            runner.runAction(action);
          }
        }}
      >
        {#if isStopping}
          <Spinner size={14} class="danger" />
        {:else if showStopIcon}
          <StopCircle size={14} />
        {:else if isRunning && (phase?.type === 'building' || action.actionType !== 'run')}
          <!-- A pinned non-run action has no serving phase: it spins, then
               reports its outcome. -->
          <Spinner size={14} />
        {:else if isRunning}
          <SineWave size={14} />
        {:else if execution?.status === 'completed'}
          <CheckCircle size={14} />
        {:else if execution?.status === 'failed'}
          <AlertCircle size={14} />
        {:else}
          <ActionIcon icon={action.icon} actionType={action.actionType} size={14} />
        {/if}
      </Button>
    {/if}
  </div>
{/if}

<style>
  /* Pinned action button — circular icon-only */
  .pinned-action-container {
    display: flex;
    align-items: center;
    overflow: hidden;
  }

  /* Pinned action pill (endpoint running state) */
  .pinned-action-pill {
    display: flex;
    align-items: center;
    height: 28px;
    background: var(--bg-hover);
    border-radius: 999px;
    overflow: hidden;
  }

  .pinned-action-pill.outline {
    background: transparent;
    border: 1px solid var(--card-border-hover);
    box-sizing: border-box;
  }

  .copy-icon-wrapper {
    position: absolute;
    display: flex;
    align-items: center;
    justify-content: center;
  }
</style>
