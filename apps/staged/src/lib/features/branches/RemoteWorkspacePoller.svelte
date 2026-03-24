<script lang="ts">
  import { onDestroy } from 'svelte';
  import * as commands from '../../api/commands';
  import type { WorkspaceStatus } from '../../types';

  interface Props {
    branchId: string;
    incomingStatus: WorkspaceStatus | null;
    status?: WorkspaceStatus | null;
    onStatusChange?: (status: WorkspaceStatus, workstationId?: number | null) => void;
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
  const RUNNING_POLL_MS = 30_000;
  const POLL_TIMEOUT_MS = 5 * 60 * 1000;

  function toWorkspaceStatus(value: string): WorkspaceStatus | null {
    return value === 'starting' ||
      value === 'running' ||
      value === 'stopped' ||
      value === 'suspended' ||
      value === 'error'
      ? value
      : null;
  }

  function setStatus(
    next: WorkspaceStatus,
    source: 'incoming' | 'poll',
    workstationId?: number | null
  ) {
    if (status === next && !workstationId) return;
    status = next;
    if (source === 'poll') {
      onStatusChange?.(next, workstationId);
    }
    console.debug(`[RemoteWorkspacePoller] branch=${branchId} source=${source} status=${next}`);
  }

  async function pollOnce() {
    if (pollInFlight) return;

    if (status === 'starting' && pollStartedAt && Date.now() - pollStartedAt > POLL_TIMEOUT_MS) {
      setStatus('error', 'poll');
      stopPolling();
      return;
    }

    pollInFlight = true;
    try {
      const result = await commands.pollWorkspaceStatus(branchId);
      const next = toWorkspaceStatus(result.status);
      console.debug(`[RemoteWorkspacePoller] branch=${branchId} poll result=${result.status}`);
      if (!next) return;
      setStatus(next, 'poll', result.workstationId);
      if (next !== 'starting' && next !== 'running') {
        stopPolling();
      } else if (next === 'running' && pollTimer) {
        // Switch from fast starting interval to slower running interval
        clearInterval(pollTimer);
        pollTimer = setInterval(() => {
          void pollOnce();
        }, RUNNING_POLL_MS);
      }
    } catch (e) {
      // Keep polling while still provisioning or running.
      if (status !== 'starting' && status !== 'running') {
        setStatus('error', 'poll');
        stopPolling();
      } else {
        console.debug(`[RemoteWorkspacePoller] branch=${branchId} poll failed while ${status}:`, e);
      }
    } finally {
      pollInFlight = false;
    }
  }

  function startPolling(intervalMs: number) {
    if (pollTimer) return;
    pollStartedAt = Date.now();
    console.debug(
      `[RemoteWorkspacePoller] branch=${branchId} start polling (interval=${intervalMs}ms)`
    );
    void pollOnce();
    pollTimer = setInterval(() => {
      void pollOnce();
    }, intervalMs);
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
      startPolling(POLL_MS);
    } else if (status === 'running') {
      startPolling(RUNNING_POLL_MS);
    } else {
      stopPolling();
    }
  });

  onDestroy(() => {
    stopPolling();
  });
</script>
