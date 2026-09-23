<!--
  DoctorCheckRow.svelte — A single row in the doctor report.

  Shows a status icon (✓ / ⚠ / ✗), the check label, a message,
  and optional action buttons: an external-link icon to open an
  install page, or a "Fix" button that runs a shell command.
-->
<script lang="ts">
  import CheckCircle from '@lucide/svelte/icons/check-circle';
  import AlertTriangle from '@lucide/svelte/icons/alert-triangle';
  import XCircle from '@lucide/svelte/icons/x-circle';
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import Wrench from '@lucide/svelte/icons/wrench';
  import ArrowUpCircle from '@lucide/svelte/icons/arrow-up-circle';
  import { openUrl, runDoctorFix } from '../../api/commands';
  import type { AgentVersionInfo, DoctorCheck } from '../../api/commands';
  import {
    doctorState,
    updateCheck,
    isReadoutActionable,
    hasActionableUpdate,
  } from './doctor.svelte';
  import {
    agentLogin,
    attachAgentLogin,
    cancelAgentLogin,
    clearAgentLogin,
    startAgentLogin,
    type AgentLoginOutcome,
  } from './agentLogin.svelte';
  import AgentLoginPrompt from './AgentLoginPrompt.svelte';
  import { closingFixDialogCancelsLogin } from './fixDialog';
  import { Button } from '$lib/components/ui/button';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import Spinner from '../../shared/Spinner.svelte';
  import AgentIcon from '../agents/AgentIcon.svelte';

  let {
    check,
    agentId,
    onFixed,
  }: {
    check: DoctorCheck;
    agentId?: string;
    onFixed?: () => void;
  } = $props();

  let fixing = $state(false);
  let fixError = $state<string | null>(null);
  let showFixDialog = $state(false);

  // The "Fix" button never handles updates — those use update commands that are
  // derived per-readout, not the static fix command.
  const FIX_TYPES = ['command', 'bridge', 'auth'] as const;
  const canFix = $derived(
    !!check.fixType &&
      (FIX_TYPES as readonly string[]).includes(check.fixType) &&
      !!check.fixCommand &&
      check.status !== 'pass'
  );

  /** Whether a readout surfaces an update badge (under its own path line). */
  function showsUpdateBadge(info: AgentVersionInfo | null): boolean {
    return info?.updateAvailable === true;
  }
  const anyUpdateBadge = $derived(showsUpdateBadge(check.main) || showsUpdateBadge(check.bridge));

  /** Update commands that will run when the user confirms (actionable only). */
  const updateCommands = $derived(
    [check.main, check.bridge].filter((r) => isReadoutActionable(r)).map((r) => r!.updateCommand!)
  );

  const canUpdate = $derived(hasActionableUpdate(check));
  const updating = $derived(doctorState.updating.includes(check.id));

  // A panel-wide "Update all" run serializes its installs; while it's in flight
  // this row's actions must stay disabled, or a user could fire a second update
  // for a check the batch hasn't reached yet and race two global installs.
  const batchUpdating = $derived(doctorState.updatingAll);

  // Show a per-row spinner while the (global, batched) freshness pass runs.
  // Skip `fail` rows — the tool isn't installed, so "checking for an update"
  // is noise — and skip rows that already surface a result (Update button or
  // badge) so the spinner and the result never display together.
  const showFreshnessSpinner = $derived(
    doctorState.freshnessLoading && check.status !== 'fail' && !canUpdate && !anyUpdateBadge
  );

  let showUpdateDialog = $state(false);
  let updateError = $state<string | null>(null);

  const isAuthFix = $derived(check.fixType === 'auth');
  /**
   * Whether the shared login record is running this check's login — whichever
   * entry point started it. For an `auth` fix this, not `fixing`, is what the
   * dialog reports: `fixing` is only this dialog's wait on that record.
   */
  const loginRunning = $derived(isAuthFix && agentLogin.running && agentLogin.checkId === check.id);
  const fixRunning = $derived(isAuthFix ? loginRunning : fixing);
  /**
   * This dialog started the login the record shows — its Run was confirmed —
   * rather than attaching on open to one already running. Decides what leaving
   * the dialog does to that login; see `closingFixDialogCancelsLogin`. Read only
   * from handlers, never rendered, so plain state.
   */
  let loginStartedHere = false;
  /**
   * Opens of the fix dialog so far. A login follower from an earlier open — one
   * this dialog detached from on close and, re-opened, attached to again — is
   * waiting on the same login as the current open's; it stands down so the end
   * is acted on once.
   */
  let fixDialogOpens = 0;

  async function promptFix() {
    if (!check.fixType) return;
    fixError = null;
    fixing = false;
    fixDialogOpens += 1;
    if (!isAuthFix) {
      showFixDialog = true;
      return;
    }
    // Don't open on the last attempt's leftovers.
    clearAgentLogin(check.id);
    showFixDialog = true;
    // A login for this check may already be running — started from the
    // session pane, from another client, or before this view reloaded. Pick it
    // up so the dialog shows its URL and code box, instead of a Run the backend
    // would answer "already running". Not this dialog's login to end.
    loginStartedHere = false;
    await followLogin(attachAgentLogin(check.id));
  }

  async function confirmFix() {
    if (!check.fixType) return;
    if (isAuthFix) {
      // The one interactive fix: it prints a sign-in URL and then waits for
      // the code that page hands back. Started through the shared login
      // record so the dialog can show both, instead of running blind and
      // expiring at the fix timeout.
      loginStartedHere = true;
      await followLogin(startAgentLogin(check.id));
      return;
    }
    fixing = true;
    fixError = null;
    try {
      // canFix guarantees fixType is one of the non-update kinds here.
      await runDoctorFix(check.id, check.fixType as 'command' | 'bridge');
      showFixDialog = false;
      onFixed?.();
    } catch (e) {
      fixError = String(e);
    } finally {
      fixing = false;
    }
  }

  /**
   * Wait on a login from this dialog: close it when the login ends, and refresh
   * the report only if it signed in. `null` is an attach that found nothing
   * running, which leaves the dialog offering Run. A follower outlived by a
   * re-open of the dialog leaves both to the current open's follower.
   */
  async function followLogin(login: Promise<AgentLoginOutcome | null>) {
    const opened = fixDialogOpens;
    fixing = true;
    fixError = null;
    let outcome: AgentLoginOutcome | null;
    try {
      outcome = await login;
    } catch (e) {
      if (opened !== fixDialogOpens) return;
      fixing = false;
      // A failed login is already rendered from the shared record — unless
      // another check's login owns that record, which is what a rejection
      // before the login even started means.
      if (agentLogin.checkId !== check.id) fixError = String(e);
      return;
    }
    if (opened !== fixDialogOpens) return;
    fixing = false;
    if (outcome === null) return;
    showFixDialog = false;
    if (outcome === 'completed') onFixed?.();
  }

  /**
   * Leave the fix dialog. The footer's Cancel, Escape and a click outside all
   * land here through `onOpenChange`. For a login this dialog started, leaving
   * is also the abort: nothing else is watching that login, and it must not
   * hold the check's login slot until doctor's fix timeout — the CLI ignores a
   * closed stdin, so only a kill ends it. A login the dialog merely attached to
   * on open is left running: the session pane that started it is still showing
   * its URL and code box, and this was only a look. An install can't be
   * cancelled yet and keeps Cancel disabled while it runs; Escape still closes
   * its dialog, and `fixing` clears when it ends.
   */
  function cancelFix() {
    if (isAuthFix) {
      if (closingFixDialogCancelsLogin({ running: loginRunning, startedHere: loginStartedHere })) {
        void cancelAgentLogin();
      }
      showFixDialog = false;
      return;
    }
    if (fixing) return;
    showFixDialog = false;
  }

  function promptUpdate() {
    if (!canUpdate) return;
    updateError = null;
    showUpdateDialog = true;
  }

  async function confirmUpdate() {
    updateError = null;
    try {
      await updateCheck(check);
      showUpdateDialog = false;
      // onFixed (runChecksAndRefresh) is the single full re-run: a base scan
      // that re-derives status/message, a chained freshness pass that clears
      // the badges, and a provider refresh. No separate freshness call needed.
      onFixed?.();
    } catch (e) {
      updateError = String(e);
    }
  }

  function cancelUpdate() {
    if (updating) return;
    showUpdateDialog = false;
  }
</script>

{#snippet updateBadge(slot: 'main' | 'bridge', info: AgentVersionInfo | null)}
  {#if info?.updateAvailable === true}
    <span class="update-badge" class:info-only={!info.updateCommand}>
      <ArrowUpCircle size={11} />
      {slot === 'bridge' ? 'Bridge update' : 'Update'} available:
      {info.installedVersion ?? '?'} → {info.latestVersion ?? '?'}
    </span>
  {/if}
{/snippet}

<div
  class="check-row"
  class:pass={check.status === 'pass'}
  class:warn={check.status === 'warn'}
  class:fail={check.status === 'fail'}
>
  <div class="status-icon">
    {#if check.status === 'pass'}
      <CheckCircle size={16} />
    {:else if check.status === 'warn'}
      <AlertTriangle size={16} />
    {:else}
      <XCircle size={16} />
    {/if}
  </div>

  <div class="check-info">
    <span class="check-label">
      {#if agentId}
        <AgentIcon id={agentId} size={16} />
      {/if}
      {check.label}
    </span>
    <span class="check-message">{check.message}</span>
    <!-- "Managed by Staged": resolved from the managed bridge shim dir,
         whose installs and updates Staged owns — no manual update nag. -->
    {#if check.path}
      {#if check.main?.bundled}
        <span class="check-path">Managed by Staged</span>
      {:else}
        <span class="check-path">{check.path}</span>
      {/if}
      {@render updateBadge('main', check.main)}
    {/if}
    {#if check.bridgePath}
      {#if check.bridge?.bundled}
        <span class="check-path">Managed by Staged</span>
      {:else}
        <span class="check-path">{check.bridgePath}</span>
      {/if}
      {@render updateBadge('bridge', check.bridge)}
    {/if}
  </div>

  {#if showFreshnessSpinner}
    <Spinner size={14} />
  {/if}

  {#if canUpdate}
    <Button variant="outline" size="sm" disabled={updating || batchUpdating} onclick={promptUpdate}>
      <ArrowUpCircle size={14} />
      {updating ? 'Updating' : 'Update'}
    </Button>
  {/if}

  {#if canFix}
    <Button variant="outline" size="sm" disabled={batchUpdating} onclick={promptFix}>
      <Wrench size={14} />
      Fix
    </Button>
  {/if}

  {#if check.fixUrl && check.status !== 'pass'}
    <Button variant="ghost" size="icon" onclick={() => openUrl(check.fixUrl!)}>
      <ExternalLink size={14} />
    </Button>
  {/if}
</div>

<!-- Every user-driven close (Cancel, Escape, a click outside) reports through
     `onOpenChange`; `cancelFix` is what decides whether a running login goes
     with it — only one this dialog started. Programmatic closes don't fire it. -->
<AlertDialog.Root bind:open={showFixDialog} onOpenChange={(open) => !open && cancelFix()}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Run fix command?</AlertDialog.Title>
      <AlertDialog.Description class="max-h-[42vh] overflow-auto whitespace-pre-line">
        {check.fixCommand}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <!-- Sign-in URL and code entry, for an `auth` fix that is running. The
         footer's Cancel is the abort here, so the prompt doesn't show its own. -->
    <AgentLoginPrompt checkId={check.id} cancellable={false} />
    {#if fixError}
      <p class="text-destructive text-sm">{fixError}</p>
    {/if}
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={!isAuthFix && fixing}>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="outline"
        disabled={fixing || loginRunning}
        onclick={(e) => {
          e.preventDefault();
          confirmFix();
        }}
      >
        {fixRunning ? 'Running' : fixError ? 'Retry' : 'Run'}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<AlertDialog.Root bind:open={showUpdateDialog}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title
        >Run update command{updateCommands.length > 1 ? 's' : ''}?</AlertDialog.Title
      >
      <AlertDialog.Description class="max-h-[42vh] overflow-auto whitespace-pre-line">
        {updateCommands.join('\n')}
      </AlertDialog.Description>
    </AlertDialog.Header>
    {#if updateError}
      <p class="text-destructive text-sm">{updateError}</p>
    {/if}
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={updating} onclick={cancelUpdate}>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="outline"
        disabled={updating}
        onclick={(e) => {
          e.preventDefault();
          confirmUpdate();
        }}
      >
        {updating ? 'Updating' : updateError ? 'Retry' : 'Update'}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<style>
  .check-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    background: var(--bg-primary);
    border-radius: 8px;
  }

  .status-icon {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .check-row.pass .status-icon {
    color: var(--color-success, #3fb950);
  }

  .check-row.warn .status-icon {
    color: var(--color-warning, #d29922);
  }

  .check-row.fail .status-icon {
    color: var(--color-danger, #f85149);
  }

  .check-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .check-label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .check-message {
    font-size: var(--size-xs);
    color: var(--text-muted);
    overflow-wrap: break-word;
    word-wrap: break-word;
  }

  .check-path {
    font-size: 10px;
    color: var(--text-faint, rgba(255, 255, 255, 0.35));
    font-family: monospace;
    overflow-wrap: break-word;
    word-wrap: break-word;
  }

  .update-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    align-self: flex-start;
    margin-top: 2px;
    font-size: 10px;
    color: var(--color-warning, #d29922);
  }

  /* When there's no runnable command, the badge is informational only. */
  .update-badge.info-only {
    color: var(--text-muted);
  }
</style>
