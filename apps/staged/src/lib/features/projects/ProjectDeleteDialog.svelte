<!--
  ProjectDeleteDialog.svelte - Shared remove-project confirmation dialog

  Mounted once in App.svelte and driven by projectActions.pendingDelete, so
  every remove-project entry point (sidebar context menu on the project and
  repos routes, landing-grid context menu, ProjectHome's top-bar button and
  shortcut) shares one confirmation flow.
-->
<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { projectDisplayName } from '../../shared/utils';
  import { projectActions } from './projectActions.svelte';
</script>

<AlertDialog.Root
  open={projectActions.pendingDelete !== null}
  onOpenChange={(v) => !v && projectActions.cancelPendingDelete()}
>
  <AlertDialog.Content>
    {#if projectActions.pendingDelete}
      <AlertDialog.Header>
        <AlertDialog.Title>Remove Project</AlertDialog.Title>
        <AlertDialog.Description>
          {`Remove "${projectDisplayName(projectActions.pendingDelete)}" from Staged? There are unmerged changes in this project's branches. Deleting this project will lose any changes not pushed to GitHub.`}
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action
          variant="destructive"
          onclick={() => projectActions.confirmPendingDelete()}
        >
          Remove
        </AlertDialog.Action>
      </AlertDialog.Footer>
    {/if}
  </AlertDialog.Content>
</AlertDialog.Root>
