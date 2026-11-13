<!--
  ConnectedAppsPanel.svelte - Settings ▸ Connected apps

  Every client approved through `stagedctl connect` that has not been
  revoked: who it is, what it reaches, whether it may write, when it last
  used the connection and when the approval lapses. Revoke drops the client's
  live credentials at once and refuses its next reconnect. The list follows
  the backend's `control-grants-changed` event (approvals, renewals,
  revocations from either side), so it needs no manual refresh.

  Above the list sits the action that puts stagedctl on the user's PATH
  (`StagedctlPathSetting`), since a client has to be able to run the CLI
  before it can ask to connect.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import Plug from '@lucide/svelte/icons/plug';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { Button } from '$lib/components/ui/button';
  import Spinner from '../../shared/Spinner.svelte';
  import * as commands from '../../api/commands';
  import { listenToEvent } from '../../transport';
  import { LatestWins } from '../../shared/latestWins';
  import { formatRelativeTime } from '../../shared/relativeTime.svelte';
  import type { ControlGrantSummary } from '../../types';
  import { accessLabel, scopeLabel } from './controlAuthorization.svelte';
  import StagedctlPathSetting from './StagedctlPathSetting.svelte';

  /** Mirrors `GRANTS_CHANGED_EVENT` in `control_server/consent.rs`. */
  const GRANTS_CHANGED_EVENT = 'control-grants-changed';

  let grants = $state<ControlGrantSummary[] | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);
  let confirming = $state<ControlGrantSummary | null>(null);
  let revoking = $state(false);
  let mounted = true;

  /**
   * Each `control-grants-changed` sends a load, and the backend reads them
   * concurrently: the load sent for the latest change stays, whichever
   * answers last.
   */
  const loadOrder = new LatestWins<'grants'>();

  async function load() {
    const ticket = loadOrder.issue();
    loading = true;
    try {
      const list = await commands.listControlGrants();
      if (!mounted || !loadOrder.admit('grants', ticket)) return;
      grants = list;
      error = null;
    } catch (e) {
      // A failure is a response too: one from a load that a later load has
      // already answered must not put its error above that load's list.
      if (!mounted || !loadOrder.admit('grants', ticket)) return;
      error = e instanceof Error ? e.message : String(e);
    } finally {
      // `loading` is about the newest load. An older one settling while that
      // is still out leaves the Refresh button disabled.
      if (mounted && loadOrder.isNewest(ticket)) loading = false;
    }
  }

  onMount(() => {
    void load();
    const unlisten = listenToEvent<void>(GRANTS_CHANGED_EVENT, () => void load());
    return () => {
      mounted = false;
      unlisten();
    };
  });

  async function revoke() {
    const grant = confirming;
    if (!grant || revoking) return;
    revoking = true;
    try {
      await commands.revokeControlGrant(grant.id);
      // The event refreshes too; this keeps the row gone even if it is late.
      grants = grants?.filter((g) => g.id !== grant.id) ?? null;
      confirming = null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      revoking = false;
    }
  }

  function expiryLabel(grant: ControlGrantSummary): string {
    const left = grant.expiresAt - Date.now();
    if (left <= 0) return 'expired';
    const days = Math.ceil(left / 86_400_000);
    return days <= 1 ? 'in under a day' : `in ${days} days`;
  }
</script>

<div class="connected-apps-panel">
  <div class="panel-intro">
    <div class="intro-copy">
      <h2>
        <Plug size={16} />
        Connected apps
      </h2>
      <p>
        Tools and agents you approved to control Staged through stagedctl from outside a session.
        Each approval lasts thirty days from its last use.
      </p>
    </div>
    <div class="header-actions">
      <Button variant="outline" size="sm" disabled={loading} onclick={() => load()}>
        <RefreshCw size={14} />
        Refresh
      </Button>
    </div>
  </div>

  <div class="panel-body">
    <StagedctlPathSetting />
    {#if error}
      <div role="alert" class="text-destructive text-sm">{error}</div>
    {/if}
    {#if grants === null && loading}
      <div class="loading-state">
        <Spinner size={24} />
        <span>Loading connected apps...</span>
      </div>
    {:else if grants && grants.length === 0}
      <div class="empty-state" data-testid="connected-apps-empty">
        No connected apps. Approvals from <code>stagedctl connect</code> appear here.
      </div>
    {:else if grants}
      <ul class="grants" aria-label="Connected apps">
        {#each grants as grant (grant.id)}
          <li class="grant" data-testid="connected-app" data-grant-id={grant.id}>
            <div class="grant-main">
              <div class="grant-title">
                <span class="grant-client">{grant.client}</span>
                {#if grant.live}
                  <span class="grant-live" title="Holds a live credential in this run of Staged"
                    >connected now</span
                  >
                {/if}
              </div>
              <div class="grant-meta">
                <span>{accessLabel(grant.access)}</span>
                <span aria-hidden="true">·</span>
                <span>{scopeLabel(grant.projectId, grant.projectName)}</span>
                <span aria-hidden="true">·</span>
                <span>
                  {grant.lastUsedAt === null
                    ? 'never used'
                    : `last used ${formatRelativeTime(grant.lastUsedAt)}`}
                </span>
                <span aria-hidden="true">·</span>
                <span>expires {expiryLabel(grant)}</span>
              </div>
            </div>
            <Button
              variant="destructive"
              size="sm"
              disabled={revoking}
              onclick={() => (confirming = grant)}
            >
              Revoke
            </Button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<AlertDialog.Root
  open={confirming !== null}
  onOpenChange={(open) => {
    if (!open && !revoking) confirming = null;
  }}
>
  <!-- Revoking is destructive though its button isn't styled so: Return
       doesn't confirm it. -->
  <AlertDialog.Content defaultAction="none">
    {#if confirming}
      <AlertDialog.Header>
        <AlertDialog.Title>Revoke access for “{confirming.client}”?</AlertDialog.Title>
        <AlertDialog.Description>
          Its stagedctl commands stop working immediately. It can ask again with
          <code>stagedctl connect</code>.
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel disabled={revoking} onclick={() => (confirming = null)}>
          Cancel
        </AlertDialog.Cancel>
        <AlertDialog.Action disabled={revoking} onclick={() => revoke()}>Revoke</AlertDialog.Action>
      </AlertDialog.Footer>
    {/if}
  </AlertDialog.Content>
</AlertDialog.Root>

<style>
  .connected-apps-panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg-primary);
  }

  .panel-intro {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 14px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .intro-copy {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }

  .panel-intro h2 {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-md);
    font-weight: 600;
  }

  .panel-intro p {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .panel-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .grants {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 640px;
    width: 100%;
    margin-inline: auto;
  }

  .grant {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    background: var(--bg-elevated);
  }

  .grant-main {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .grant-title {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .grant-client {
    font-size: var(--size-sm);
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .grant-live {
    font-size: var(--size-xs);
    color: var(--ui-accent);
    white-space: nowrap;
  }

  .grant-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .loading-state,
  .empty-state {
    color: var(--text-muted);
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 160px;
    font-size: var(--size-sm);
    text-align: center;
  }

  code {
    font-size: 0.9em;
  }

  @media (max-width: 920px) {
    .panel-intro {
      flex-direction: column;
      align-items: flex-start;
    }
  }
</style>
