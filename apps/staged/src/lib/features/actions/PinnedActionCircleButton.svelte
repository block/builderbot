<!--
  PinnedActionCircleButton.svelte — the shape a pinned action takes whenever
  it isn't a serving run action with an endpoint: a circular button showing the
  action's icon, its progress while running, or its outcome once it's done.

  Purely presentational, like PinnedActionEndpointPill: the parent derives the
  execution state and decides what pressing the button does.
-->
<script lang="ts">
  import { Button } from '$lib/components/ui/button';
  import type { ProjectAction } from '../../api/commands';
  import type { ActionStatus } from './actions';
  import ActionIcon from './ActionIcon.svelte';
  import ActionStatusIcon from './ActionStatusIcon.svelte';
  import { actionStatusLabels } from './actionStatusLabels';

  interface Props {
    action: ProjectAction;
    /** The execution's status, when the action has an execution at all. */
    status?: ActionStatus;
    /** The execution is live. */
    running: boolean;
    /** A stop has been requested and the process hasn't exited yet. */
    stopping: boolean;
    /** The button is currently offering to stop the action (alt held). */
    showStop: boolean;
    /** A run action past its build phase, serving. */
    serving: boolean;
    /** Outline surface theme (the repo card's badge-hued variant). */
    outline: boolean;
    onPress: () => void;
  }

  let { action, status, running, stopping, showStop, serving, outline, onPress }: Props = $props();

  let labels = $derived(
    actionStatusLabels({ actionName: action.name, stopping, showStop, running, status })
  );
</script>

<Button
  variant="ghost"
  class={[
    'size-7 rounded-full [&_svg]:!size-3.5',
    outline
      ? 'border border-[var(--card-border-hover)] bg-transparent hover:bg-[var(--card-bg-strong)]'
      : 'border-0 bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)]',
    running &&
      (outline
        ? 'text-muted-foreground'
        : 'bg-[var(--bg-hover)] text-muted-foreground hover:bg-[var(--bg-elevated)]'),
    status === 'completed' &&
      (outline
        ? 'border-[var(--status-added)] text-[var(--status-added)]'
        : 'bg-[var(--bg-hover)] text-[var(--status-added)] hover:bg-[var(--bg-hover)]'),
    status === 'failed' &&
      (outline
        ? 'border-destructive text-destructive'
        : 'bg-[var(--bg-hover)] text-destructive hover:bg-[var(--bg-hover)]'),
    stopping &&
      (outline ? 'opacity-60 hover:bg-transparent' : 'opacity-60 hover:bg-[var(--bg-elevated)]'),
    showStop && (outline ? 'border-destructive text-destructive' : 'text-destructive'),
  ]}
  title={labels.title}
  aria-label={labels.ariaLabel}
  onclick={onPress}
>
  <ActionStatusIcon {stopping} {showStop} {serving} {running} {status} size={14}>
    {#snippet idle()}
      <ActionIcon icon={action.icon} actionType={action.actionType} size={14} />
    {/snippet}
  </ActionStatusIcon>
</Button>
