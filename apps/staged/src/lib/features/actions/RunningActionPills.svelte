<!--
  RunningActionPills.svelte — pill row for a scope's running actions,
  excluding the pinned ones (each of which PinnedActionButton renders).

  Each pill shows the action's live status (spinner, sine wave for a serving
  run action, check/alert on completion), opens the output modal on click, and
  stops the action on alt-click. Driven entirely by an ActionRunner.

  Status icon and tooltip come from ActionStatusIcon / actionStatusLabels,
  shared with the pinned-action buttons. A pill takes the tooltip only: its own
  text already names the action, so an aria-label would just talk over it.

  variant selects the surface theme: 'default' is the branch card's elevated
  neutral pill; 'outline' is a clear background outlined with the host card's
  theme, reading the --accent / --card-border-hover / --card-bg-strong custom
  properties the repo card sets from its badge hue.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { cubicOut } from 'svelte/easing';
  import StopCircle from '@lucide/svelte/icons/stop-circle';
  import { Button } from '$lib/components/ui/button';
  import type { ActionRunner } from './actionRunner.svelte';
  import ActionStatusIcon from './ActionStatusIcon.svelte';
  import { actionStatusLabels } from './actionStatusLabels';
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
  {@const serving =
    isRunning && !!phase && phase.type !== 'building' && execution.actionType === 'run'}
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
      title={actionStatusLabels({
        actionName: execution.actionName,
        stopping: isStopping,
        showStop: showStopIcon,
        running: isRunning,
        status: execution.status,
      }).title}
      onclick={() => {
        if (isRunning && altKey.held && !isStopping) {
          runner.stopAction(execution.executionId, execution.actionName);
        } else {
          runner.showOutput(execution);
        }
      }}
    >
      <ActionStatusIcon
        stopping={isStopping}
        showStop={showStopIcon}
        {serving}
        running={isRunning}
        status={execution.status}
        size={12}
      >
        {#snippet idle()}
          <StopCircle size={12} />
        {/snippet}
      </ActionStatusIcon>
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
