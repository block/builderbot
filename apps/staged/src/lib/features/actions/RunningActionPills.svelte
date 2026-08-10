<!--
  RunningActionPills.svelte — pill row for a scope's running actions,
  excluding the primary run action (which PrimaryRunActionButton renders).

  Each pill shows the action's live status (spinner, sine wave for a serving
  run action, check/alert on completion), opens the output modal on click, and
  stops the action on alt-click. Driven entirely by an ActionRunner.

  variant selects the surface theme: 'default' is the branch card's elevated
  neutral pill; 'outline' is a clear background outlined with the host card's
  theme, reading the --accent / --card-border-hover / --card-bg-strong custom
  properties the repo card sets from its badge hue.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { cubicOut } from 'svelte/easing';
  import CheckCircle from '@lucide/svelte/icons/check-circle';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import StopCircle from '@lucide/svelte/icons/stop-circle';
  import Spinner from '../../shared/Spinner.svelte';
  import SineWave from '../../shared/SineWave.svelte';
  import { Button } from '$lib/components/ui/button';
  import type { ActionRunner } from './actionRunner.svelte';
  import { altKey, trackAltKey } from './altKey.svelte';

  interface Props {
    runner: ActionRunner;
    variant?: 'default' | 'outline';
  }

  let { runner, variant = 'default' }: Props = $props();

  let outline = $derived(variant === 'outline');

  onMount(() => trackAltKey());

  // Custom transition combining slide and fade effects
  function slideAndFade(
    node: Element,
    { duration = 300, axis = 'x' }: { duration?: number; axis?: 'x' | 'y' } = {}
  ) {
    const style = getComputedStyle(node);
    const opacity = +style.opacity;
    const primaryDimension = axis === 'y' ? 'height' : 'width';
    const primaryDimensionValue = parseFloat(style[primaryDimension]);
    const paddingStart = axis === 'y' ? 'paddingTop' : 'paddingLeft';
    const paddingEnd = axis === 'y' ? 'paddingBottom' : 'paddingRight';
    const marginStart = axis === 'y' ? 'marginTop' : 'marginLeft';
    const marginEnd = axis === 'y' ? 'marginBottom' : 'marginRight';

    return {
      duration,
      easing: cubicOut,
      css: (t: number) => {
        return [
          `overflow: hidden`,
          `opacity: ${t * opacity}`,
          `${primaryDimension}: ${t * primaryDimensionValue}px`,
          `padding-${paddingStart.replace(/[A-Z]/g, (m) => `-${m.toLowerCase()}`)}: ${t * parseFloat(style[paddingStart])}px`,
          `padding-${paddingEnd.replace(/[A-Z]/g, (m) => `-${m.toLowerCase()}`)}: ${t * parseFloat(style[paddingEnd])}px`,
          `margin-${marginStart.replace(/[A-Z]/g, (m) => `-${m.toLowerCase()}`)}: ${t * parseFloat(style[marginStart])}px`,
          `margin-${marginEnd.replace(/[A-Z]/g, (m) => `-${m.toLowerCase()}`)}: ${t * parseFloat(style[marginEnd])}px`,
        ].join(';');
      },
    };
  }
</script>

{#each runner.secondaryRunningActions as execution (execution.executionId)}
  {@const isRunning = execution.status === 'running'}
  {@const isStopping = runner.stoppingExecutions.has(execution.executionId)}
  {@const showStopIcon = altKey.held && isRunning && !isStopping}
  {@const phase = runner.runPhases.get(execution.executionId)}
  <div
    class="running-action-container"
    class:fading={execution.fading}
    transition:slideAndFade={{ duration: 300, axis: 'x' }}
  >
    <Button
      variant="ghost"
      class={[
        'h-auto whitespace-nowrap rounded-full border px-3 py-1.5 gap-1.5 text-xs text-foreground [&_svg]:!size-3',
        outline
          ? 'bg-transparent border-[var(--card-border-hover)] hover:bg-[var(--card-bg-strong)] hover:border-[var(--accent)]'
          : 'bg-[var(--bg-elevated)] border-[var(--border-muted)] hover:bg-[var(--bg-hover)] hover:border-[var(--border-focus)]',
        execution.status === 'completed' &&
          'border-[var(--status-added)] text-[var(--status-added)]',
        execution.status === 'failed' && 'border-destructive text-destructive',
        isStopping &&
          (outline
            ? 'opacity-60 hover:bg-transparent hover:border-[var(--card-border-hover)]'
            : 'opacity-60 hover:bg-[var(--bg-elevated)] hover:border-[var(--border-muted)]'),
        showStopIcon && 'border-destructive text-destructive',
      ]}
      title={isStopping
        ? 'Stopping…'
        : showStopIcon
          ? `Stop ${execution.actionName}`
          : isRunning
            ? `View output for ${execution.actionName}`
            : execution.status === 'completed'
              ? `${execution.actionName} completed`
              : execution.status === 'failed'
                ? `${execution.actionName} failed`
                : execution.actionName}
      onclick={() => {
        if (isRunning && altKey.held && !isStopping) {
          runner.stopAction(execution.executionId, execution.actionName);
        } else {
          runner.showOutput(execution);
        }
      }}
    >
      {#if isStopping}
        <Spinner size={12} class="danger" />
      {:else if showStopIcon}
        <StopCircle size={12} />
      {:else if isRunning && phase && phase.type !== 'building' && execution.actionType === 'run'}
        <SineWave size={12} />
      {:else if isRunning}
        <Spinner size={12} />
      {:else if execution.status === 'completed'}
        <CheckCircle size={12} />
      {:else if execution.status === 'failed'}
        <AlertCircle size={12} />
      {:else}
        <StopCircle size={12} />
      {/if}
      {execution.actionName}
    </Button>
  </div>
{/each}

<style>
  .running-action-container {
    display: flex;
    align-items: center;
    gap: 4px;
    overflow: hidden;
  }

  .running-action-container.fading {
    opacity: 0;
    transform: scale(0.95);
    transition:
      opacity 0.3s ease,
      transform 0.3s ease;
  }
</style>
