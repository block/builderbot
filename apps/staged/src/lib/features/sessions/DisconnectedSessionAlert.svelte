<!--
  DisconnectedSessionAlert.svelte — the chat pane's notice for a
  `disconnected` session: a Blox session that lost its connection mid-turn,
  whose agent may still be working on the workstation.

  Says what happened, where the backend's attempts to catch up stand (catching
  up, stopping, or why the last attempt couldn't and when the next is due),
  and offers Check now (reconcile immediately) and Stop. There is no Resume
  here on purpose: a resumed turn would run beside the one that may still be
  going on the workstation.
-->
<script lang="ts">
  import CloudOff from '@lucide/svelte/icons/cloud-off';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import CircleStop from '@lucide/svelte/icons/circle-stop';
  import * as Alert from '$lib/components/ui/alert';
  import { Button } from '$lib/components/ui/button';
  import Spinner from '../../shared/Spinner.svelte';
  import { minuteNow } from '../../shared/relativeTime.svelte';
  import {
    DISCONNECTED_ALERT_MESSAGE,
    disconnectedAlertStatus,
  } from '../../shared/disconnectedSession';
  import type { SessionReconcileEvent } from '../../types';

  interface Props {
    /** Where reconciling the session stands, if the backend has said. */
    reconcile: SessionReconcileEvent | null;
    /** A Check now was just requested and the backend hasn't answered yet. */
    checkRequested?: boolean;
    /** A Stop request is in flight. */
    stopping?: boolean;
    onCheckNow: () => void;
    onStop: () => void;
  }

  let { reconcile, checkRequested = false, stopping = false, onCheckNow, onStop }: Props = $props();

  let status = $derived(disconnectedAlertStatus(reconcile, minuteNow.now(), checkRequested));
  let stopBusy = $derived(stopping || status.busy === 'stopping');

  let root = $state<HTMLElement | null>(null);

  // What the status line says, minus its "Next check in N min." countdown:
  // that ticks every minute and must not pull a reader back down.
  let statusKey = $derived(status.busy ?? (status.text ? (reconcile?.reason?.kind ?? '') : null));

  // The alert lands at the end of the transcript, which can sit below the
  // fold; bring it (and its Check now / Stop buttons) into view when it
  // appears and whenever the status line changes. `nearest` is a no-op when
  // it is already visible, and nothing here takes focus.
  $effect(() => {
    void statusKey;
    root?.scrollIntoView?.({ block: 'nearest' });
  });
</script>

<Alert.Root bind:ref={root} class="mt-3" data-testid="disconnected-session-alert">
  <CloudOff class="text-[var(--ui-warning)]" />
  <Alert.Description>
    <div class="disconnected-body">
      <span>{DISCONNECTED_ALERT_MESSAGE}</span>
      {#if status.text}
        <span
          class="disconnected-status"
          title={status.detail ?? undefined}
          data-testid="disconnected-session-status"
          aria-live="polite"
        >
          {#if status.busy}
            <Spinner size={12} />
          {/if}
          {status.text}
        </span>
      {/if}
      <div class="disconnected-actions">
        <Button
          variant="outline"
          size="xs"
          onclick={onCheckNow}
          disabled={status.busy !== null || stopBusy}
          title="Check the workstation now"
        >
          <RefreshCw size={14} />
          Check now
        </Button>
        <Button variant="outline" size="xs" onclick={onStop} disabled={stopBusy}>
          {#if stopBusy}
            <Spinner size={14} />
          {:else}
            <CircleStop size={14} />
          {/if}
          Stop
        </Button>
      </div>
    </div>
  </Alert.Description>
</Alert.Root>

<style>
  .disconnected-body {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
  }

  .disconnected-status {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .disconnected-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
</style>
