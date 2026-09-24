<!--
  DoctorCheckRow.svelte — A single row in the doctor report.

  Shows a status icon (✓ / ⚠ / ✗), the check label, a message, a version
  readout per binary behind an agent check (main agent / ACP bridge, see
  DoctorVersionReadout), and optional action buttons: an external-link icon
  to open an install page, a "Fix" button that runs a shell command, or an
  "Update" button for a readout with an actionable update.
-->
<script lang="ts">
  import CheckCircle from '@lucide/svelte/icons/check-circle';
  import AlertTriangle from '@lucide/svelte/icons/alert-triangle';
  import XCircle from '@lucide/svelte/icons/x-circle';
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import Wrench from '@lucide/svelte/icons/wrench';
  import ArrowUpCircle from '@lucide/svelte/icons/arrow-up-circle';
  import { openUrl, runDoctorFix } from '../../api/commands';
  import type { DoctorCheck } from '../../api/commands';
  import {
    doctorState,
    updateCheck,
    isReadoutActionable,
    hasActionableUpdate,
  } from './doctor.svelte';
  import DoctorVersionReadout from './DoctorVersionReadout.svelte';
  import { updateBadge } from './versionReadout';
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

  /**
   * An agent check fronts one or two binaries — the agent CLI (`main`) and its
   * ACP bridge (`bridge`) — each with its own version readout. Non-agent checks
   * (git, gh, the Node runtime) have neither and show a plain path.
   */
  const isAgentCheck = $derived(!!check.main || !!check.bridge);

  /** Whether a readout surfaces an update badge (under its own readout line). */
  const anyUpdateBadge = $derived(
    !!updateBadge('main', check.main) || !!updateBadge('bridge', check.bridge)
  );

  /** The readouts whose update runs when the user confirms (actionable only). */
  const actionableReadouts = $derived(
    [check.main, check.bridge].filter((r) => isReadoutActionable(r)).map((r) => r!)
  );
  /** What the confirmation shows: the command per readout, or for a Staged-
   *  managed install the description of what the managed installer will do. */
  const updateCommands = $derived(actionableReadouts.map((r) => r.updateCommand!));
  /**
   * Every actionable readout is Staged-managed: no shell command runs, the
   * backend reinstalls Staged's private copy, so the dialog says so instead of
   * asking to "run a command".
   *
   * `bundled` misses one case on a build that manages the Claude and Codex
   * bridges: a copy of one of them that resolved elsewhere on PATH (a user
   * install, before the launch reconcile lands) is not bundled, yet the backend
   * still routes its update to the managed installer and describes it that way
   * in `updateCommand`. Such a row gets the "run update command?" header over a
   * managed-install body. Telling the two apart needs a per-readout
   * "managed action" flag on `AgentVersionInfo`, which is a doctor crate change.
   */
  const managedUpdate = $derived(
    actionableReadouts.length > 0 && actionableReadouts.every((r) => r.bundled === true)
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
   * This dialog asked for the login the record shows — its Run was confirmed —
   * rather than attaching on open to one already running. Only what the dialog
   * knows on its own: whether that request began a run or re-attached to a
   * login someone else started is the record's `origin`, and leaving the
   * dialog reads both — see `closingFixDialogCancelsLogin`. Read only from
   * handlers, never rendered, so plain state.
   */
  let loginRequestedHere = false;
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
    loginRequestedHere = false;
    await followLogin(attachAgentLogin(check.id));
  }

  async function confirmFix() {
    if (!check.fixType) return;
    if (isAuthFix) {
      // The one interactive fix: it prints a sign-in URL and then waits for
      // the code that page hands back. Started through the shared login
      // record so the dialog can show both, instead of running blind and
      // expiring at the fix timeout. Whether the backend in fact starts one or
      // answers "already running" is on the record, not known here.
      loginRequestedHere = true;
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
   * running, which leaves the dialog offering Run.
   *
   * Two followers stand down rather than act. One outlived by a re-open of the
   * dialog leaves the end to the current open's follower, which waits on the
   * same login. One whose dialog was left before the login ended — detached from
   * it on the way out — has handed the login back to whoever is still watching
   * it, the session pane that started it, and that watcher re-runs the checks;
   * a refresh from here as well would be two full scans at once. A login this
   * dialog started and left is on its way out too (leaving cancelled it), and a
   * cancelled end refreshes nothing.
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
    if (!showFixDialog) return;
    showFixDialog = false;
    if (outcome === 'completed') onFixed?.();
  }

  /**
   * Leave the fix dialog. The footer's Cancel, Escape and a click outside all
   * land here through `onOpenChange`. For a login this dialog started, leaving
   * is also the abort: nothing else is watching that login, and it must not
   * hold the check's login slot until doctor's fix timeout — the CLI ignores a
   * closed stdin, so only a kill ends it. A login the dialog merely attached to
   * is left running: the session pane that started it is still showing its URL
   * and code box, and this was only a look. That holds whether the dialog
   * attached on open or its own Run was answered "already running" — the record
   * knows which, this dialog only what it asked for. An install can't be
   * cancelled yet and keeps Cancel disabled while it runs; Escape still closes
   * its dialog, and `fixing` clears when it ends.
   */
  function cancelFix() {
    if (isAuthFix) {
      const cancels = closingFixDialogCancelsLogin({
        running: loginRunning,
        requestedHere: loginRequestedHere,
        origin: agentLogin.origin,
      });
      if (cancels) void cancelAgentLogin();
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
    <!-- One readout per resolved binary: label, "Managed by Staged" or the
         path, the installed version, and the update badge. For a managed
         Claude/Codex install both readouts describe the same executable — the
         ACP package vendors the agent — and only the ACP one can update. -->
    {#if isAgentCheck}
      {#if check.path}
        <DoctorVersionReadout
          kind="main"
          path={check.path}
          info={check.main}
          loading={doctorState.freshnessLoading}
        />
      {/if}
      {#if check.bridgePath}
        <DoctorVersionReadout
          kind="bridge"
          path={check.bridgePath}
          info={check.bridge}
          loading={doctorState.freshnessLoading}
        />
      {/if}
    {:else if check.path}
      <span class="check-path">{check.path}</span>
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
     with it — only one this dialog started, by the backend's account.
     Programmatic closes don't fire it. -->
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
      <AlertDialog.Title>
        {#if managedUpdate}
          Update Staged's managed install?
        {:else}
          Run update command{updateCommands.length > 1 ? 's' : ''}?
        {/if}
      </AlertDialog.Title>
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
</style>
