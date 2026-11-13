<!--
  ControlAuthorizationDialog.svelte - "Allow <client> to control Staged?"

  Mounted once in App.svelte (desktop main window only) and driven by
  controlAuthorization.pending: a `stagedctl connect` request names the
  client, what it would reach and whether it may write, plus the requesting
  process as information — with a notice when that process's parent has
  already exited, the shape a request backgrounded from an agent session
  takes. Approve / Deny answer the waiting CLI; closing the dialog any other
  way counts as Deny.
-->
<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { accessLabel, controlAuthorization, scopeLabel } from './controlAuthorization.svelte';

  const request = $derived(controlAuthorization.pending);
  const peerLine = $derived.by(() => {
    if (!request || request.peerPid === null) return null;
    const parent = request.peerParent ? ` (${request.peerParent})` : '';
    return `Requested by process ${request.peerPid}${parent} on this Mac.`;
  });
</script>

<AlertDialog.Root
  open={request !== null}
  onOpenChange={(open) => {
    if (!open && controlAuthorization.pending && !controlAuthorization.busy) {
      void controlAuthorization.deny();
    }
  }}
>
  <!-- Approving grants access, and this dialog opens unprompted: Return
       doesn't approve. -->
  <AlertDialog.Content defaultAction="none">
    {#if request}
      <AlertDialog.Header>
        <AlertDialog.Title>Allow “{request.client}” to control Staged?</AlertDialog.Title>
        <AlertDialog.Description>
          It is asking for {accessLabel(request.access)} access to {scopeLabel(
            request.projectId,
            request.projectName
          )} through stagedctl. Approval lasts thirty days from its last use and can be revoked any time
          under Settings ▸ Connected apps.
        </AlertDialog.Description>
      </AlertDialog.Header>
      <dl class="details" data-testid="control-authorization-details">
        <dt>Access</dt>
        <dd data-testid="control-authorization-access">{accessLabel(request.access)}</dd>
        <dt>Scope</dt>
        <dd data-testid="control-authorization-scope">
          {scopeLabel(request.projectId, request.projectName)}
        </dd>
        {#if peerLine}
          <dt>Source</dt>
          <dd data-testid="control-authorization-peer">{peerLine}</dd>
        {/if}
      </dl>
      {#if request.orphaned}
        <p class="orphaned" role="note" data-testid="control-authorization-orphaned">
          The asking process's parent has already exited, so where it was started from cannot be
          shown. It may have been backgrounded from an agent session; approve only if you ran this
          command yourself.
        </p>
      {/if}
      {#if controlAuthorization.queuedCount > 0}
        <p class="queued">
          {controlAuthorization.queuedCount} more request{controlAuthorization.queuedCount === 1
            ? ''
            : 's'} waiting.
        </p>
      {/if}
      <AlertDialog.Footer>
        <AlertDialog.Cancel
          disabled={controlAuthorization.busy}
          onclick={() => controlAuthorization.deny()}
        >
          Deny
        </AlertDialog.Cancel>
        <AlertDialog.Action
          disabled={controlAuthorization.busy}
          onclick={() => controlAuthorization.approve()}
        >
          Approve
        </AlertDialog.Action>
      </AlertDialog.Footer>
    {/if}
  </AlertDialog.Content>
</AlertDialog.Root>

<style>
  .details {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.25rem 0.75rem;
    margin: 0;
    font-size: 0.875rem;
  }

  .details dt {
    color: var(--text-secondary);
  }

  .details dd {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .orphaned {
    margin: 0;
    padding: 0.5rem 0.75rem;
    border-radius: 0.375rem;
    background: var(--ui-warning-bg);
    color: var(--ui-warning);
    font-size: 0.8125rem;
  }

  .queued {
    margin: 0;
    font-size: 0.8125rem;
    color: var(--text-secondary);
  }
</style>
