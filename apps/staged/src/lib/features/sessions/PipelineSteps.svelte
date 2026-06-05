<!--
  PipelineSteps.svelte — Renders pipeline step progress for a session.

  Shows a compact vertical list of steps with status icons, labels, and
  expandable output. Subscribes to `pipeline-step-changed` events for
  live updates.
-->
<script lang="ts">
  import { onMount, onDestroy, untrack } from 'svelte';
  import Bot from '@lucide/svelte/icons/bot';
  import Check from '@lucide/svelte/icons/check';
  import X from '@lucide/svelte/icons/x';
  import Circle from '@lucide/svelte/icons/circle';
  import Minus from '@lucide/svelte/icons/minus';
  import Spinner from '../../shared/Spinner.svelte';
  import { Button } from '$lib/components/ui/button';
  import * as Tooltip from '$lib/components/ui/tooltip';
  import { listenToEvent, type UnlistenFn } from '../../transport';
  import type { PipelineExecution, PipelineStepStatus, PipelineStepPayload } from '../../types';
  import { formatPipelineStepDuration } from './pipelineDuration';

  interface Props {
    sessionId: string;
    pipeline: PipelineExecution;
  }

  let { sessionId, pipeline }: Props = $props();

  let steps = $state<PipelineStepStatus[]>([]);
  let expandedSteps = $state<Set<number>>(new Set());
  let now = $state(Date.now());

  // Sync steps from the pipeline prop, but only when the prop has more progress
  // than local state. This avoids overwriting live updates from the event
  // listener with a stale prop value from a cached session fetch.
  // We use untrack() when reading `steps` to avoid creating a dependency loop
  // (this effect should only re-run when pipeline.steps changes, not when we
  // write to local steps).
  $effect(() => {
    const propSteps = pipeline.steps;
    const localSteps = untrack(() => steps);
    if (localSteps.length === 0 || propSteps.length !== localSteps.length) {
      steps = propSteps;
    } else {
      // Only sync if the prop has more progress (more non-pending steps).
      const propProgress = propSteps.filter((s) => s.status !== 'pending').length;
      const localProgress = localSteps.filter((s) => s.status !== 'pending').length;
      if (propProgress > localProgress) {
        steps = propSteps;
      }
    }
  });

  $effect(() => {
    const hasRunningStep = steps.some(
      (step) => step.status === 'running' && step.startedAt !== null && step.completedAt === null
    );
    if (!hasRunningStep) return;

    now = Date.now();
    const interval = setInterval(() => {
      now = Date.now();
    }, 1000);

    return () => clearInterval(interval);
  });

  let unlisten: UnlistenFn | null = null;

  onMount(() => {
    unlisten = listenToEvent<PipelineStepPayload>('pipeline-step-changed', (payload) => {
      if (payload.sessionId !== sessionId) return;
      const { stepIndex, status, output, error, startedAt, completedAt } = payload;
      if (stepIndex < steps.length) {
        steps[stepIndex] = {
          ...steps[stepIndex],
          status,
          output: output ?? steps[stepIndex].output,
          error: error ?? steps[stepIndex].error,
          startedAt: startedAt ?? steps[stepIndex].startedAt,
          completedAt: completedAt ?? steps[stepIndex].completedAt,
        };
        // Auto-expand failed steps, except AI handoff boundary rows with no details.
        if (
          status === 'failed' &&
          (steps[stepIndex].stepType !== 'ai_handoff' ||
            steps[stepIndex].output ||
            steps[stepIndex].error)
        ) {
          expandedSteps = new Set([...expandedSteps, stepIndex]);
        }
      }
    });
  });

  onDestroy(() => {
    unlisten?.();
  });

  function toggleStep(index: number) {
    const next = new Set(expandedSteps);
    if (next.has(index)) {
      next.delete(index);
    } else {
      next.add(index);
    }
    expandedSteps = next;
  }

  function showsAiHandoffIcon(step: PipelineStepStatus): boolean {
    return (
      step.stepType === 'ai_handoff' &&
      (step.status === 'pending' || step.status === 'running' || step.status === 'succeeded')
    );
  }
</script>

<div class="pipeline-steps">
  {#each steps as step, idx}
    <div class="step" class:expanded={expandedSteps.has(idx)}>
      <Tooltip.Root disabled={step.stepType !== 'ai_handoff'}>
        <Tooltip.Trigger>
          {#snippet child({ props })}
            <Button
              {...props}
              variant="ghost"
              type="button"
              class="flex h-auto w-full items-center justify-start gap-2 rounded px-0 py-1 text-left text-[13px] font-normal text-foreground shadow-none hover:bg-[var(--bg-hover)] hover:text-foreground"
              onclick={() => toggleStep(idx)}
            >
              <span class="step-icon">
                {#if showsAiHandoffIcon(step)}
                  <Bot size={14} class="icon-ai-handoff" />
                {:else if step.status === 'pending'}
                  <Circle size={14} class="icon-pending" />
                {:else if step.status === 'running'}
                  <Spinner size={14} />
                {:else if step.status === 'succeeded'}
                  <Check size={14} class="icon-succeeded" />
                {:else if step.status === 'failed'}
                  <X size={14} class="icon-failed" />
                {:else if step.status === 'skipped'}
                  <Minus size={14} class="icon-skipped" />
                {/if}
              </span>
              <span class="step-label">{step.label}</span>
              {#if step.stepType === 'ai_handoff'}
                <span class="step-kind">AI</span>
              {:else}
                <span class="step-duration">
                  {formatPipelineStepDuration(step.startedAt, step.completedAt, now)}
                </span>
              {/if}
            </Button>
          {/snippet}
        </Tooltip.Trigger>
        <Tooltip.Content>AI session starts here</Tooltip.Content>
      </Tooltip.Root>
      {#if expandedSteps.has(idx) && (step.output || step.error)}
        <div class="step-output">
          <pre>{step.output ?? ''}{#if step.error}
              {step.error}{/if}</pre>
        </div>
      {/if}
    </div>
  {/each}
</div>

<style>
  .pipeline-steps {
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .step {
    margin-bottom: 2px;
  }

  .step-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    flex-shrink: 0;
  }

  .step-icon :global(.icon-pending) {
    color: var(--text-faint);
  }

  .step-icon :global(.icon-succeeded) {
    color: var(--ui-success);
  }

  .step-icon :global(.icon-failed) {
    color: var(--ui-danger);
  }

  .step-icon :global(.icon-skipped) {
    color: var(--text-faint);
  }

  .step-icon :global(.icon-ai-handoff) {
    color: var(--text-accent);
  }

  .step-label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .step-duration {
    color: var(--text-muted);
    font-size: 12px;
    flex-shrink: 0;
  }

  .step-kind {
    color: var(--text-accent);
    font-size: 10px;
    font-weight: 600;
    line-height: 1;
    letter-spacing: 0;
    flex-shrink: 0;
  }

  .step-output {
    margin-left: 26px;
    margin-bottom: 6px;
    background: var(--bg-deepest);
    border-radius: 4px;
    padding: 8px 10px;
    overflow-x: auto;
    max-height: 200px;
    overflow-y: auto;
  }

  .step-output pre {
    margin: 0;
    font-size: 11px;
    line-height: 1.4;
    color: var(--text-muted);
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
