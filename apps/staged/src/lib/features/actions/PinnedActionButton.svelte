<!--
  PinnedActionButton.svelte — one pinned action's button in a card header,
  driven by an ActionRunner.

  Derives the action's live state from the runner and picks the shape that
  state calls for: PinnedActionCircleButton (the action's icon, a spinner while
  building, a sine wave while a run action serves, check/alert on completion),
  or PinnedActionEndpointPill once a serving run action reports its endpoint.
  Click opens the output modal; alt-click stops the running execution. A header
  renders one of these per pinned action, each tracking its own execution —
  pinned non-run actions simply never reach the serving or endpoint states.

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
  import { onMount } from 'svelte';
  import { slide } from 'svelte/transition';
  import type { ProjectAction } from '../../api/commands';
  import type { ActionRunner } from './actionRunner.svelte';
  import { altKey, trackAltKey } from './altKey.svelte';
  import PinnedActionCircleButton from './PinnedActionCircleButton.svelte';
  import PinnedActionEndpointPill from './PinnedActionEndpointPill.svelte';

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

  let execution = $derived(runner.executionFor(action.id));
  let isRunning = $derived(execution?.status === 'running');
  let isStopping = $derived(!!execution && runner.stoppingExecutions.has(execution.executionId));
  let showStopIcon = $derived(altKey.held && isRunning && !isStopping);
  let phase = $derived(execution ? runner.runPhases.get(execution.executionId) : undefined);
  let endpoint = $derived(phase?.type === 'running' ? phase.endpoint : null);
  let copyUrl = $derived(endpoint && canResolveEndpoint ? getEndpointCopyUrl(endpoint) : '');
  // A pinned non-run action has no serving phase: it spins, then reports its
  // outcome.
  let serving = $derived(isRunning && phase?.type !== 'building' && action.actionType === 'run');
  let showPill = $derived(isRunning && !!endpoint && canResolveEndpoint);

  onMount(() => trackAltKey());

  function press(): void {
    if (showStopIcon && execution) {
      runner.stopAction(execution.executionId, action.name);
    } else if (execution && (isRunning || isStopping)) {
      runner.showOutput(execution);
    } else {
      runner.runAction(action);
    }
  }
</script>

{#if show}
  <div
    class="pinned-action-container"
    in:slide={{ duration: 300, axis: 'x' }}
    out:slide={{ duration: 300, axis: 'x' }}
  >
    {#if showPill}
      <PinnedActionEndpointPill
        actionName={action.name}
        {copyUrl}
        stopping={isStopping}
        showStop={showStopIcon}
        {outline}
        onPress={press}
      />
    {:else}
      <PinnedActionCircleButton
        {action}
        status={execution?.status}
        running={isRunning}
        stopping={isStopping}
        showStop={showStopIcon}
        {serving}
        {outline}
        onPress={press}
      />
    {/if}
  </div>
{/if}

<style>
  .pinned-action-container {
    display: flex;
    align-items: center;
    overflow: hidden;
  }
</style>
