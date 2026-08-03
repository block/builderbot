<script lang="ts">
  import Clock from '@lucide/svelte/icons/clock';
  import CircleAlert from '@lucide/svelte/icons/circle-alert';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import CircleDot from '@lucide/svelte/icons/circle-dot';
  import CircleSlash from '@lucide/svelte/icons/circle-slash';
  import type { RichToolItem } from '../acpTranscript';

  interface Props {
    statusTone: RichToolItem['statusTone'];
  }

  let { statusTone }: Props = $props();
</script>

<span
  class="tool-status-dot"
  class:status-running={statusTone === 'running'}
  class:status-success={statusTone === 'success'}
  class:status-danger={statusTone === 'danger'}
  class:status-cancelled={statusTone === 'cancelled'}
>
  {#if statusTone === 'running'}
    <Clock size={11} />
  {:else if statusTone === 'success'}
    <CircleCheck size={11} />
  {:else if statusTone === 'danger'}
    <CircleAlert size={11} />
  {:else if statusTone === 'cancelled'}
    <CircleSlash size={11} />
  {:else}
    <CircleDot size={11} />
  {/if}
</span>

<style>
  .tool-status-dot {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 12px;
    height: 12px;
    flex-shrink: 0;
    color: var(--text-faint);
  }

  .tool-status-dot.status-running {
    color: var(--ui-warning);
  }

  .tool-status-dot.status-success {
    color: var(--ui-success, var(--ui-accent));
  }

  .tool-status-dot.status-danger {
    color: var(--ui-danger);
  }

  .tool-status-dot.status-cancelled {
    color: var(--text-muted);
  }
</style>
