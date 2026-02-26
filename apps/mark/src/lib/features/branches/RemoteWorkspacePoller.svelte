<script lang="ts">
  import { onDestroy } from 'svelte';
  import * as commands from '../../api/commands';
  import type { WorkspaceStatus } from '../../types';

  interface Props {
    branchId: string;
    incomingStatus: WorkspaceStatus | null;
    status?: WorkspaceStatus | null;
    onStatusChange?: (status: WorkspaceStatus) => void;
  }

  let {
    branchId,
    incomingStatus,
    status = $bindable<WorkspaceStatus | null>(incomingStatus),
    onStatusChange,
  }: Props = $props();

  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let pollStartedAt: number | null = null;
  let pollInFlight = false;
  const POLL_MS = 3000;
  const POLL_TIMEOUT_MS = 5 * 60 * 1000;

  function toWorkspaceStatus(value: string): WorkspaceStatus | null {
    return value === 'starting' || value === 'running' || value === 'stopped' || value === 'error'
      ? value
      : null;
  }

  function setStatus(next: WorkspaceStatus, source: 'incoming' | 'poll') {
    if (status === next) return;
    status = next;
    if (source === 'poll') {
      onStatusChange?.(next);
    }
    console.debug(`[RemoteWorkspacePoller] branch=${branchId} source=${source} status=${next}`);
  }

  async function pollOnce() {
    if (pollInFlight) return;

    if (pollStartedAt && Date.now() - pollStartedAt > POLL_TIMEOUT_MS) {
      setStatus('error', 'poll');
      stopPolling();
      return;
    }

    pollInFlight = true;
    try {
      const raw = await commands.pollWorkspaceStatus(branchId);
      const next = toWorkspaceStatus(raw);
      console.debug(`[RemoteWorkspacePoller] branch=${branchId} poll result=${raw}`);
      if (!next) return;
      setStatus(next, 'poll');
      if (next !== 'starting') {
        stopPolling();
      }
    } catch (e) {
      // Keep polling while still provisioning.
      if (status !== 'starting') {
        setStatus('error', 'poll');
        stopPolling();
      } else {
        console.debug(`[RemoteWorkspacePoller] branch=${branchId} poll failed while starting:`, e);
      }
    } finally {
      pollInFlight = false;
    }
  }

  function startPolling() {
    if (pollTimer) return;
    pollStartedAt = Date.now();
    console.debug(`[RemoteWorkspacePoller] branch=${branchId} start polling`);
    void pollOnce();
    pollTimer = setInterval(() => {
      void pollOnce();
    }, POLL_MS);
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
      console.debug(`[RemoteWorkspacePoller] branch=${branchId} stop polling`);
    }
    pollInFlight = false;
  }

  // Keep local state synced to canonical backend snapshots.
  $effect(() => {
    if (incomingStatus && incomingStatus !== 'starting') {
      setStatus(incomingStatus, 'incoming');
      return;
    }
    if (incomingStatus === 'starting' && !status) {
      status = 'starting';
    }
  });

  $effect(() => {
    if (status === 'starting') {
      startPolling();
    } else {
      stopPolling();
    }
  });

  onDestroy(() => {
    stopPolling();
  });
</script>
