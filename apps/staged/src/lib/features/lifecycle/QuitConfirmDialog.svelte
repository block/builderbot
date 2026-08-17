<!--
  QuitConfirmDialog.svelte - Confirmation for quitting with sessions running

  Mounted once in App.svelte and driven by the quitPrompt store, which
  quitListener.ts fills from the backend's `app:quit-requested` event. Confirming
  hands back to `confirm_quit`, which stops the sessions and then exits the
  process — so the dialog stays up in its "Stopping sessions…" state rather than
  closing on an app that is still shutting down.
-->
<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { quitPrompt } from '../../stores/quitPrompt.svelte';
  import type { ActiveSessionInfo } from '../../types';
  import { quitPromptDescription, quitSessionLabel } from './quitPromptCopy';

  /**
   * Where a session is running, as the user knows it: its branch, or its project
   * for project-level sessions. Either may be missing when the payload arrives
   * before that project's branches are hydrated, which the copy handles.
   */
  function sessionLocation(session: ActiveSessionInfo): string | null {
    const projectName =
      projectsDataStore.projects.find((project) => project.id === session.projectId)?.name ?? null;

    if (!session.branchId || !session.projectId) return projectName;

    const branch = projectsDataStore.branchesByProject
      .get(session.projectId)
      ?.find((candidate) => candidate.id === session.branchId);

    return branch?.branchName ?? projectName;
  }

  const description = $derived(
    quitPromptDescription(
      (quitPrompt.payload?.sessions ?? []).map((session) =>
        quitSessionLabel(session, sessionLocation(session))
      ),
      quitPrompt.payload?.runningActionCount ?? 0
    )
  );
</script>

<AlertDialog.Root open={quitPrompt.open} onOpenChange={(v) => !v && quitPrompt.cancel()}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Quit Staged?</AlertDialog.Title>
      <AlertDialog.Description>{description}</AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={quitPrompt.stopping}>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={quitPrompt.stopping}
        onclick={() => quitPrompt.confirm()}
      >
        {quitPrompt.stopping ? 'Stopping sessions…' : 'Quit & Stop Sessions'}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
