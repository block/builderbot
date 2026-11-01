<!--
  ResumeOnLaunchDialog.svelte - "Resume interrupted sessions?" at launch

  Mounted once in App.svelte (desktop main window only) and driven by
  resumeOnLaunch.pending. Lists the sessions the last quit interrupted, repo
  sessions under their branch and project sessions under their project, and
  offers Resume all / Dismiss. Closing the dialog any other way counts as
  Dismiss, so the offer is always recorded as handled.
-->
<script lang="ts">
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { resumeOnLaunch } from './resumeOnLaunch.svelte';
  import {
    dispositionNote,
    groupInterruptedSessions,
    interruptedSummary,
    resumableSessionIds,
  } from './resumeOnLaunchGroups';

  const groups = $derived(
    resumeOnLaunch.pending ? groupInterruptedSessions(resumeOnLaunch.pending) : []
  );
  const summary = $derived(
    resumeOnLaunch.pending ? interruptedSummary(resumeOnLaunch.pending) : ''
  );
  const canResume = $derived(
    resumeOnLaunch.pending ? resumableSessionIds(resumeOnLaunch.pending).length > 0 : false
  );
</script>

<AlertDialog.Root
  open={resumeOnLaunch.pending !== null}
  onOpenChange={(open) => {
    if (!open && resumeOnLaunch.pending && !resumeOnLaunch.busy) void resumeOnLaunch.dismiss();
  }}
>
  <AlertDialog.Content>
    {#if resumeOnLaunch.pending}
      <AlertDialog.Header>
        <AlertDialog.Title>Resume interrupted sessions?</AlertDialog.Title>
        <AlertDialog.Description>{summary}</AlertDialog.Description>
      </AlertDialog.Header>
      <div class="groups" role="list" aria-label="Interrupted sessions">
        {#each groups as group (group.key)}
          <section class="group" role="listitem">
            <h3 class="group-heading">{group.heading}</h3>
            <ul class="sessions">
              {#each group.sessions as session (session.sessionId)}
                {@const note = dispositionNote(session)}
                <li class="session" data-disposition={session.disposition}>
                  <span class="session-label">{session.label}</span>
                  {#if note}
                    <span class="session-note">({note})</span>
                  {/if}
                </li>
              {/each}
            </ul>
          </section>
        {/each}
      </div>
      <AlertDialog.Footer>
        <AlertDialog.Cancel disabled={resumeOnLaunch.busy} onclick={() => resumeOnLaunch.dismiss()}>
          Dismiss
        </AlertDialog.Cancel>
        <AlertDialog.Action
          disabled={resumeOnLaunch.busy || !canResume}
          onclick={() => resumeOnLaunch.resumeAll()}
        >
          Resume all
        </AlertDialog.Action>
      </AlertDialog.Footer>
    {/if}
  </AlertDialog.Content>
</AlertDialog.Root>

<style>
  .groups {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    max-height: 40vh;
    overflow-y: auto;
    font-size: 0.875rem;
  }

  .group-heading {
    margin: 0 0 0.25rem;
    font-size: 0.75rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.02em;
    color: var(--text-secondary);
  }

  .sessions {
    margin: 0;
    padding-left: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }

  .session-note {
    color: var(--text-secondary);
  }

  .session[data-disposition='manual'] .session-label {
    color: var(--text-secondary);
  }
</style>
